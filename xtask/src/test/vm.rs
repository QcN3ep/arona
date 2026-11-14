// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::{Case, resources::Resources};
use crate::{Target, qemu_command};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::Path,
    process::{Child, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub(super) struct Process(pub Child);

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub(super) fn list(
    target: Target,
    root: &Path,
    kernel: &Path,
    directory: &Path,
) -> Result<Vec<Case>, String> {
    collect(
        target,
        root,
        kernel,
        directory,
        None,
        &mut Resources::default(),
        Duration::from_secs(30),
    )
}

pub(super) fn run(
    target: Target,
    root: &Path,
    kernel: &Path,
    directory: &Path,
    case: &Case,
    resources: &mut Resources,
    timeout: Duration,
) -> Result<(), String> {
    collect(
        target,
        root,
        kernel,
        directory,
        Some(case),
        resources,
        timeout,
    )
    .map(|_| ())
}

fn collect(
    target: Target,
    root: &Path,
    kernel: &Path,
    directory: &Path,
    case: Option<&Case>,
    resources: &mut Resources,
    timeout: Duration,
) -> Result<Vec<Case>, String> {
    let mut command = qemu_command(target, root);
    command.arg("-kernel").arg(kernel);
    if matches!(target, Target::Riscv64) {
        command.args([
            "-bios",
            "default",
            "-global",
            "virtio-mmio.force-legacy=false",
        ]);
    }
    command.args(["-rtc", "base=2000-02-29T23:59:59,clock=vm", "-no-reboot"]);
    command.args(&resources.arguments);
    if let Some(path) = &resources.initrd {
        command.arg("-initrd").arg(path);
    }
    fs::write(
        directory.join("command.json"),
        serde_json::to_vec_pretty(
            &std::iter::once(command.get_program())
                .chain(command.get_args())
                .map(|arg| arg.to_string_lossy())
                .collect::<Vec<_>>(),
        )
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(File::create(directory.join("qemu.log")).map_err(|error| error.to_string())?);
    let mut process = Process(
        command
            .spawn()
            .map_err(|error| format!("could not start QEMU: {error}"))?,
    );
    let mut input = process.0.stdin.take().unwrap();
    let mut output = process.0.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    let reader = thread::spawn(move || {
        let mut buffer = [0; 8192];
        loop {
            match output.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    if sender.send(Ok(buffer[..count].to_vec())).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = sender.send(Err(error.to_string()));
                    break;
                }
            }
        }
    });
    let result = observe(
        &mut process.0,
        &mut input,
        &receiver,
        directory,
        case,
        resources,
        timeout,
    );
    drop(input);
    drop(process);
    reader.join().map_err(|_| "QEMU console reader panicked")?;
    result.map_err(|error| format!("{error}; see {}", directory.join("serial.log").display()))
}

fn observe(
    process: &mut Child,
    input: &mut impl Write,
    receiver: &mpsc::Receiver<Result<Vec<u8>, String>>,
    directory: &Path,
    case: Option<&Case>,
    resources: &mut Resources,
    timeout: Duration,
) -> Result<Vec<Case>, String> {
    let mut log = File::create(directory.join("serial.log")).map_err(|error| error.to_string())?;
    let started = Instant::now();
    let mut output = Vec::new();
    let mut selected = false;
    let mut cases = Vec::new();
    let mut stimulus = 0;
    let mut cursor = 0;
    let mut fatal = None;
    loop {
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(chunk) => {
                let bytes = chunk?;
                log.write_all(&bytes).map_err(|error| error.to_string())?;
                output.extend(bytes.into_iter().filter(|byte| *byte != b'\r'));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => (),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let status = process.wait().map_err(|error| error.to_string())?;
                if resources.shutdown
                    && selected
                    && status.success()
                    && contains(
                        &output,
                        format!("KTEST RUN {}\n", case.unwrap().name).as_bytes(),
                    )
                {
                    return Ok(cases);
                }
                return Err(format!("QEMU exited before the test completed: {status}"));
            }
        }
        if !selected && contains(&output, b"KTEST READY\n") {
            cases = parse_cases(&output)?;
            if let Some(case) = case {
                if !cases.contains(case) {
                    return Err(format!("test image does not declare {}", case.name));
                }
                writeln!(input, "{}", case.name).map_err(|error| error.to_string())?;
            } else {
                writeln!(input, "--list").map_err(|error| error.to_string())?;
            }
            input.flush().map_err(|error| error.to_string())?;
            selected = true;
        }
        while let Some((marker, bytes)) = resources.inputs.get(stimulus) {
            let Some(position) = find(&output[cursor..], marker) else {
                break;
            };
            input
                .write_all(bytes)
                .and_then(|()| input.flush())
                .map_err(|error| error.to_string())?;
            cursor += position + marker.len();
            stimulus += 1;
        }
        if let Some(case) = case {
            if let Some(position) = find(&output, format!("KTEST RUN {}\n", case.name).as_bytes()) {
                let body = &output[position..];
                if case.expected.is_empty()
                    && contains(body, format!("KTEST PASS {}\n", case.name).as_bytes())
                {
                    if stimulus != resources.inputs.len() {
                        return Err(
                            "test completed without consuming all console inputs".to_owned()
                        );
                    }
                    return Ok(cases);
                }
                if !case.expected.is_empty()
                    && case
                        .expected
                        .iter()
                        .all(|marker| contains(body, marker.as_bytes()))
                {
                    return Ok(cases);
                }
            }
        } else if selected && contains(&output, b"KTEST LISTED\n") {
            return Ok(cases);
        }
        let diagnostics = if let Some(position) = find(&output, b"KTEST READY\n") {
            &output[position..]
        } else {
            &output[..find(&output, b"KTEST CASE ").unwrap_or(output.len())]
        };
        if [
            b"tests failed:".as_slice(),
            b"PANIC:",
            b"panicked at",
            b"FATAL TRAP",
            b"DMA domain poisoned",
        ]
        .iter()
        .any(|marker| contains(diagnostics, marker))
        {
            let since = fatal.get_or_insert_with(Instant::now);
            if since.elapsed() >= Duration::from_millis(250) {
                return Err("guest reported an unexpected failure".to_owned());
            }
        }
        if started.elapsed() >= timeout {
            return Err(format!(
                "test timed out after {} seconds",
                timeout.as_secs()
            ));
        }
    }
}

fn parse_cases(output: &[u8]) -> Result<Vec<Case>, String> {
    let mut cases = Vec::new();
    for line in String::from_utf8_lossy(output).lines() {
        let Some(record) = line.strip_prefix("KTEST CASE ") else {
            continue;
        };
        let mut fields = record.split('\t');
        let name = fields.next().unwrap();
        if name.is_empty() || cases.iter().any(|case: &Case| case.name == name) {
            return Err("empty or duplicate test name".to_owned());
        }
        cases.push(Case {
            name: name.to_owned(),
            expected: fields.map(str::to_owned).collect(),
        });
    }
    if cases.is_empty() {
        return Err("test image declared no cases".to_owned());
    }
    cases.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(cases)
}

fn find(bytes: &[u8], needle: &[u8]) -> Option<usize> {
    bytes.windows(needle.len()).position(|part| part == needle)
}

fn contains(bytes: &[u8], needle: &[u8]) -> bool {
    find(bytes, needle).is_some()
}
