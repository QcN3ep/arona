// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Collect and analyze instruction counts from named performance cases and system images.

mod artifacts;
mod cases;
mod diff;
mod report;
mod system;

use super::{
    Target, build_std, cargo_command, execute, linux_test, qemu_command, target_directory,
    workspace_root,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    fs::{self, File},
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const WINDOWS: [&str; 9] = [
    "empty",
    "straight",
    "loop",
    "call",
    "retry",
    "interrupt",
    "retranslation",
    "source",
    "mappings",
];
const PASSED: &str = "Perf calibration passed";
const FAILED: &str = "Perf calibration failed:";

struct Options {
    case: &'static str,
    input: system::Input,
    target: Target,
    release: Option<bool>,
    artifacts: Option<PathBuf>,
    profile: bool,
    timeout: Duration,
    filter: String,
    list: bool,
    iterations: Option<u64>,
    selection: Option<cases::Selection>,
}

impl Options {
    fn parse(arguments: impl Iterator<Item = OsString>) -> Result<Self, String> {
        let mut arguments = arguments.map(|arg| {
            arg.into_string()
                .map_err(|_| "perf arguments must be valid UTF-8".to_owned())
        });
        let target = Target::parse(&arguments.next().transpose()?.ok_or_else(super::usage)?)?;
        let mut options = Self {
            case: "kperf",
            input: system::Input::default(),
            target,
            release: None,
            artifacts: None,
            profile: false,
            timeout: Duration::from_secs(30),
            filter: String::new(),
            list: false,
            iterations: None,
            selection: None,
        };
        while let Some(argument) = arguments.next().transpose()? {
            match argument.as_str() {
                "--release" => options.release = Some(true),
                "--artifacts" => {
                    options.artifacts = Some(
                        arguments
                            .next()
                            .transpose()?
                            .ok_or("missing artifacts directory")?
                            .into(),
                    );
                }
                "--case" => {
                    options.case = match arguments.next().transpose()?.as_deref() {
                        Some("kperf") => "kperf",
                        Some("calibration") => "calibration",
                        Some("system") => "system",
                        _ => return Err("--case supports kperf, calibration, or system".to_owned()),
                    };
                }
                "--kernel" | "--initramfs" | "--start" | "--end" | "--success" | "--workload"
                | "--units" | "--image" => {
                    let value = arguments
                        .next()
                        .transpose()?
                        .ok_or_else(|| format!("missing {argument} value"))?;
                    options.input.set(&argument, value)?;
                }
                "--mode" => match arguments.next().transpose()?.as_deref() {
                    Some("count") => options.profile = false,
                    Some("profile") => options.profile = true,
                    _ => return Err("--mode supports count or profile".to_owned()),
                },
                "--list" => options.list = true,
                "--filter" => {
                    options.filter = arguments.next().transpose()?.ok_or("missing case filter")?;
                }
                "--iterations" => {
                    options.iterations = Some(
                        arguments
                            .next()
                            .transpose()?
                            .ok_or("missing iterations")?
                            .parse::<u64>()
                            .ok()
                            .filter(|count| *count > 0)
                            .ok_or("iterations must be a positive integer")?,
                    );
                }
                "--timeout" => {
                    let seconds = arguments
                        .next()
                        .transpose()?
                        .ok_or("missing timeout")?
                        .parse::<u32>()
                        .map_err(|_| "timeout must be a positive number of seconds")?;
                    if seconds == 0 {
                        return Err("timeout must be positive".to_owned());
                    }
                    options.timeout = Duration::from_secs(u64::from(seconds));
                }
                _ => return Err(format!("unknown perf option: {argument}")),
            }
        }
        if !options.input.is_empty() && (options.case != "system" || options.artifacts.is_some()) {
            return Err(
                "system input options require --case system without --artifacts".to_owned(),
            );
        }
        if (options.case != "kperf" || options.artifacts.is_some())
            && (options.list || !options.filter.is_empty() || options.iterations.is_some())
        {
            return Err("--list, --filter, and --iterations require fresh kperf cases".to_owned());
        }
        Ok(options)
    }
}

struct Window {
    name: &'static str,
    start: u64,
    end: u64,
    expected: Option<u64>,
    start_image: Option<usize>,
    end_image: Option<usize>,
}

pub(super) fn run(mut arguments: impl Iterator<Item = OsString>) -> Result<(), String> {
    match arguments.next().as_deref().and_then(|value| value.to_str()) {
        Some("report") => return report::run(arguments),
        Some("diff") => return diff::run(arguments),
        Some("run") => (),
        _ => return Err(super::usage()),
    }
    let options = Options::parse(arguments)?;
    let root = workspace_root()?;
    if options.case == "kperf" && options.artifacts.is_none() {
        return cases::run(options, &root);
    }
    execute_run(&options, &root)
}

fn execute_run(options: &Options, root: &Path) -> Result<(), String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let name = format!("{}-{stamp}-{}", options.target.triple(), std::process::id());
    let directory = target_directory(root).join("perf").join(&name);
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let mut metadata = json!({
        "status": "preparing", "target": options.target.triple(),
        "case": options.case, "mode": if options.profile { "profile" } else { "count" }, "plugin_api": 7,
        "timeout_seconds": options.timeout.as_secs(),
    });
    save_metadata(&directory, &metadata)?;
    println!("Performance results: {}", directory.display());
    let outcome = collect(options, root, &directory, stamp, &mut metadata);
    metadata["status"] = json!(if outcome.is_ok() {
        "complete"
    } else {
        "failed"
    });
    metadata["error"] = json!(outcome.as_ref().err());
    save_metadata(&directory, &metadata)?;
    outcome.map_err(|error| format!("{error}; results: {}", directory.display()))
}

fn collect(
    options: &Options,
    root: &Path,
    directory: &Path,
    stamp: u128,
    metadata: &mut Value,
) -> Result<(), String> {
    let manifest = artifacts::prepare(options, root, directory)?;
    let saved = directory.join("artifacts");
    let (windows, context) = read_windows(&saved.join("symbols.elf"), &manifest)?;
    let plugin = build_plugin(root, directory)?;
    let mut command = qemu_command(options.target, root);
    let version = command_output(Command::new(command.get_program()).arg("--version"))?;
    let control = env::temp_dir().join(format!("arona-perf-{stamp}-{}", std::process::id()));
    if matches!(options.target, Target::Riscv64) {
        command.arg("-bios").arg(saved.join("firmware.bin"));
    }
    if options.case != "calibration" {
        metadata["measurement"] = manifest["measurement"].clone();
    }
    if options.case == "system" {
        command.arg("-initrd").arg(saved.join("initramfs.cpio"));
        command.args(["-device", "virtio-rng-pci,disable-legacy=on"]);
    }
    command
        .arg("-kernel")
        .arg(saved.join("guest.elf"))
        .arg("-plugin")
        .arg(plugin_options(
            &plugin,
            directory,
            &windows,
            options.profile,
            context,
            &manifest,
        )?)
        .arg("-qmp")
        .arg(format!("unix:{}/qmp,server=on,wait=off", control.display()));
    metadata["release"] = manifest["release"].clone();
    metadata["artifacts"] = json!({
        "manifest": "artifacts/manifest.json",
        "sha256": artifacts::digest(&saved.join("manifest.json"))?,
        "reused_from": options.artifacts,
    });
    metadata["qemu_version"] = json!(version.trim());
    metadata["collector"] = json!({
        "plugin_sha256": artifacts::digest(&plugin)?,
        "source_sha256": artifacts::digest(&directory.join("count.c"))?,
        "context_header_sha256": artifacts::digest(&directory.join("context.h"))?,
        "system_header_sha256": artifacts::digest(&directory.join("system.h"))?,
        "compiler": command_output(Command::new(env::var_os("CC").unwrap_or_else(|| "cc".into())).arg("--version"))?,
    });
    metadata["command"] = json!(command.get_program().to_string_lossy());
    metadata["arguments"] = json!(
        command
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
    );
    metadata["status"] = json!("running");
    save_metadata(directory, metadata)?;
    let success = if options.case != "calibration" {
        manifest["measurement"]["success"]
            .as_str()
            .ok_or("missing completion marker")?
    } else {
        PASSED
    };
    let selection = if options.case == "kperf" {
        Some(cases::selection(&manifest)?)
    } else {
        None
    };
    measure(
        command,
        control,
        directory,
        options.timeout,
        success,
        selection,
    )?;
    validate_counts(directory, &windows)?;
    let counts = report::read_json(&directory.join("counts.json"))?;
    for window in counts["windows"]
        .as_array()
        .ok_or("missing plugin windows")?
    {
        println!(
            "{}: {} instructions",
            window["name"].as_str().ok_or("missing window name")?,
            window["instructions"]
        );
        if options.case != "calibration" {
            let units = manifest["measurement"]["units"]
                .as_u64()
                .ok_or("missing work units")?;
            let count = window["instructions"]
                .as_u64()
                .ok_or("missing instruction count")?;
            println!(
                "{units} work units, {} instructions per unit",
                count as f64 / units as f64
            );
        }
    }
    metadata["counts_sha256"] = json!(artifacts::digest(&directory.join("counts.json"))?);
    if options.profile {
        report::generate(directory, metadata)?;
    }
    Ok(())
}

fn calibration_command(options: &Options, root: &Path) -> Command {
    let mut command = cargo_command(root.to_path_buf());
    build_std(&mut command);
    command.args([
        "build",
        "--locked",
        "-p",
        "arona-arch",
        "--features",
        "baremetal",
        "--test",
        "perf",
        "--target",
        options.target.triple(),
        "--message-format=json",
    ]);
    let profile = if options.release.unwrap_or(false) {
        "RELEASE"
    } else {
        "DEV"
    };
    command.env(format!("CARGO_PROFILE_{profile}_DEBUG"), "2");
    command.env(format!("CARGO_PROFILE_{profile}_STRIP"), "none");
    if options.release.unwrap_or(false) {
        command.arg("--release");
    }
    command
}

fn build_plugin(root: &Path, directory: &Path) -> Result<PathBuf, String> {
    let flags =
        command_output(Command::new("pkg-config").args(["--cflags", "--libs", "glib-2.0"]))?;
    let output = directory.join("count.so");
    fs::copy(root.join("xtask/perf/count.c"), directory.join("count.c"))
        .map_err(|error| error.to_string())?;
    fs::copy(
        root.join("xtask/perf/context.h"),
        directory.join("context.h"),
    )
    .map_err(|error| error.to_string())?;
    fs::copy(root.join("xtask/perf/system.h"), directory.join("system.h"))
        .map_err(|error| error.to_string())?;
    let mut command = Command::new(env::var_os("CC").unwrap_or_else(|| "cc".into()));
    command
        .args([
            "-shared",
            "-fPIC",
            "-fvisibility=hidden",
            "-O2",
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Werror",
        ])
        .args(["-MD", "-MF"])
        .arg(directory.join("count.d"))
        .arg(directory.join("count.c"))
        .arg("-o")
        .arg(&output)
        .args(flags.split_whitespace());
    execute(command).map_err(|error| {
        format!("plugin build failed (requires QEMU plugin API 7 headers and GLib): {error}")
    })?;
    Ok(output)
}

fn command_output(command: &mut Command) -> Result<String, String> {
    let output = command.output().map_err(|error| {
        format!(
            "failed to start {}: {error}",
            command.get_program().to_string_lossy()
        )
    })?;
    if !output.status.success() {
        return Err(format!(
            "{} failed: {}",
            command.get_program().to_string_lossy(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout).map_err(|error| error.to_string())
}

fn read_windows(executable: &Path, manifest: &Value) -> Result<(Vec<Window>, u64), String> {
    let output = command_output(
        Command::new("llvm-nm")
            .args(["--format=posix", "--defined-only", "--no-demangle"])
            .arg(executable),
    )?;
    let mut symbols = BTreeMap::new();
    for line in output.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() >= 3 {
            symbols.insert(
                fields[0],
                u64::from_str_radix(fields[2], 16).map_err(|error| error.to_string())?,
            );
        }
    }
    if manifest["case"] != "calibration" {
        let directory = executable
            .parent()
            .ok_or("missing saved artifact directory")?;
        let window = system::window(directory, manifest, &symbols)?;
        for (pc, index) in [
            (window.start, window.start_image),
            (window.end, window.end_image),
        ] {
            let path = index.map_or_else(
                || executable.to_path_buf(),
                |index| directory.join(format!("images/{index}/symbols.elf")),
            );
            let image = artifacts::inspect(&path)?;
            if !image["ProgramHeaders"]
                .as_array()
                .ok_or("missing ELF segments")?
                .iter()
                .any(|header| {
                    let segment = &header["ProgramHeader"];
                    segment["Type"]["Value"] == 1
                        && segment["Flags"]["Value"]
                            .as_u64()
                            .is_some_and(|flags| flags & 1 != 0)
                        && segment["VirtualAddress"]
                            .as_u64()
                            .zip(segment["FileSize"].as_u64())
                            .is_some_and(|(base, size)| {
                                pc.checked_sub(base).is_some_and(|offset| offset < size)
                            })
                })
            {
                return Err(format!(
                    "boundary 0x{pc:x} is outside executable segments in {}",
                    path.display()
                ));
            }
        }
        return Ok((vec![window], 0));
    }
    let symbol = |name, suffix| {
        symbols
            .get(format!("__arona_perf_{name}_{suffix}").as_str())
            .copied()
            .ok_or_else(|| format!("missing calibration symbol: {name}_{suffix}"))
    };
    let windows = WINDOWS
        .into_iter()
        .map(|name| {
            Ok(Window {
                name,
                start: symbol(name, "start")?,
                end: symbol(name, "end")?,
                expected: if matches!(name, "source" | "mappings") {
                    None
                } else {
                    Some(symbol(name, "expected")?)
                },
                start_image: None,
                end_image: None,
            })
        })
        .collect::<Result<_, String>>()?;
    Ok((windows, symbol("context", "address")?))
}

fn plugin_options(
    plugin: &Path,
    directory: &Path,
    windows: &[Window],
    profile: bool,
    context: u64,
    manifest: &Value,
) -> Result<String, String> {
    let escape = |path: &Path| {
        path.to_str()
            .map(|text| text.replace(',', ",,"))
            .ok_or_else(|| "QEMU plugin paths must be valid UTF-8".to_owned())
    };
    let mut options = format!(
        "file={},output={}",
        escape(plugin)?,
        escape(&directory.join("counts.json"))?
    );
    for window in windows {
        use std::fmt::Write;
        write!(
            options,
            ",window={}:{:x}:{:x}:{}:{}",
            window.name,
            window.start,
            window.end,
            window
                .start_image
                .map_or_else(|| "-1".to_owned(), |image| image.to_string()),
            window
                .end_image
                .map_or_else(|| "-1".to_owned(), |image| image.to_string())
        )
        .map_err(|error| error.to_string())?;
    }
    if manifest["case"] != "calibration" {
        options.push_str(",case=system");
        options.push_str(",kernel=");
        options.push_str(&escape(&directory.join("artifacts/guest.elf"))?);
        for image in manifest["images"]
            .as_array()
            .ok_or("missing saved component images")?
        {
            let saved = image["directory"]
                .as_str()
                .ok_or("missing image directory")?;
            options.push_str(",image=");
            options.push_str(&escape(
                &directory.join("artifacts").join(saved).join("guest.elf"),
            )?);
        }
    }
    if profile {
        use std::fmt::Write;
        options.push_str(",profile=on");
        if manifest["case"] == "calibration" {
            write!(options, ",context={context:x}").map_err(|error| error.to_string())?;
        }
    }
    Ok(options)
}

struct Emulator {
    child: Child,
    control: PathBuf,
}

impl Emulator {
    fn stop(&mut self) -> Result<(), String> {
        let stream =
            UnixStream::connect(self.control.join("qmp")).map_err(|error| error.to_string())?;
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .map_err(|error| error.to_string())?;
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .map_err(|error| error.to_string())?;
        let mut reader = BufReader::new(stream);
        let mut greeting = String::new();
        reader
            .read_line(&mut greeting)
            .map_err(|error| error.to_string())?;
        if serde_json::from_str::<Value>(&greeting).map_err(|error| error.to_string())?["QMP"]
            .is_null()
        {
            return Err("missing QMP greeting".to_owned());
        }
        writeln!(reader.get_mut(), "{{\"execute\":\"qmp_capabilities\"}}")
            .map_err(|error| error.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if Instant::now() >= deadline {
                return Err("QMP capabilities timed out".to_owned());
            }
            let mut line = String::new();
            if reader
                .read_line(&mut line)
                .map_err(|error| error.to_string())?
                == 0
            {
                return Err("QMP closed before acknowledging capabilities".to_owned());
            }
            let reply: Value = serde_json::from_str(&line).map_err(|error| error.to_string())?;
            if reply.get("return").is_some() {
                break;
            }
            if reply.get("error").is_some() {
                return Err(format!("QMP error: {reply}"));
            }
        }
        writeln!(reader.get_mut(), "{{\"execute\":\"quit\"}}")
            .map_err(|error| error.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().map_err(|error| error.to_string())? {
                return if status.success() {
                    Ok(())
                } else {
                    Err(format!("QEMU exited with {status}"))
                };
            }
            thread::sleep(Duration::from_millis(20));
        }
        Err("QEMU did not exit after QMP quit".to_owned())
    }
}

impl Drop for Emulator {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_file(self.control.join("qmp"));
        let _ = fs::remove_dir(&self.control);
    }
}

fn measure(
    mut command: Command,
    control: PathBuf,
    directory: &Path,
    timeout: Duration,
    success: &str,
    selection: Option<(&str, u64)>,
) -> Result<(), String> {
    let serial = directory.join("serial.log");
    command
        .stdin(Stdio::piped())
        .stdout(File::create(&serial).map_err(|error| error.to_string())?)
        .stderr(File::create(directory.join("qemu.log")).map_err(|error| error.to_string())?);
    fs::create_dir(&control).map_err(|error| error.to_string())?;
    let child = command.spawn().map_err(|error| {
        let _ = fs::remove_dir(&control);
        error.to_string()
    })?;
    let mut emulator = Emulator { child, control };
    let mut selected = selection.is_none();
    let deadline = Instant::now() + timeout;
    loop {
        let bytes = fs::read(&serial).map_err(|error| error.to_string())?;
        let log = String::from_utf8_lossy(&bytes);
        if !selected && log.lines().any(|line| line == "KTEST READY") {
            let (name, iterations) = selection.unwrap();
            cases::select(
                &bytes,
                name,
                iterations,
                emulator.child.stdin.as_mut().unwrap(),
            )?;
            selected = true;
        }
        let failures = [
            FAILED,
            "PANIC:",
            "panicked at",
            "KPERF failed:",
            "FATAL TRAP",
        ];
        let failed = log
            .lines()
            .filter(|line| !line.starts_with("KTEST CASE "))
            .any(|line| failures.iter().any(|marker| line.contains(marker)));
        let passed = selected
            && log.lines().any(|line| {
                if selection.is_some() {
                    line == success
                } else {
                    line.contains(success)
                }
            })
            && selection.is_none_or(|(_, iterations)| {
                log.lines()
                    .any(|line| line == format!("KPERF UNITS {iterations}"))
            });
        if let Some(status) = emulator
            .child
            .try_wait()
            .map_err(|error| error.to_string())?
        {
            return if status.success() && passed && !failed {
                Ok(())
            } else {
                Err(format!(
                    "QEMU exited before measurement completed: {status}; see qemu.log"
                ))
            };
        }
        if failed || passed {
            let stopped = emulator.stop();
            return if failed {
                Err("guest failed; see serial.log".to_owned())
            } else {
                stopped
            };
        }
        if Instant::now() >= deadline {
            let _ = emulator.stop();
            return Err(format!(
                "measurement timed out after {} seconds",
                timeout.as_secs()
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn validate_counts(directory: &Path, expected: &[Window]) -> Result<(), String> {
    let bytes = fs::read(directory.join("counts.json"))
        .map_err(|error| format!("cannot read instruction counts: {error}; see qemu.log"))?;
    let counts: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if counts["plugin_api"] != 7
        || counts["valid"] != true
        || counts["metric"] != "guest_instruction_dispatches"
    {
        return Err(format!("invalid plugin result: {}", counts["error"]));
    }
    let windows = counts["windows"]
        .as_array()
        .ok_or("missing plugin windows")?;
    if windows.len() != expected.len() {
        return Err("unexpected measurement windows".to_owned());
    }
    for (window, expected) in windows.iter().zip(expected) {
        let count = window["instructions"]
            .as_u64()
            .ok_or("invalid instruction count")?;
        let matches = match expected.expected {
            Some(expected) => count == expected,
            None => count > 0,
        };
        if window["name"] != expected.name
            || window["start"] != format!("0x{:x}", expected.start)
            || window["end"] != format!("0x{:x}", expected.end)
            || window.get("start_image") != Some(&json!(expected.start_image))
            || window.get("end_image") != Some(&json!(expected.end_image))
            || window["starts"] != 1
            || window["ends"] != 1
            || !matches
        {
            return Err(format!(
                "{}: expected {} instructions, got {}",
                expected.name,
                expected.expected.map_or_else(
                    || "a positive number of".to_owned(),
                    |count| count.to_string()
                ),
                window["instructions"]
            ));
        }
    }
    Ok(())
}

fn save_metadata(directory: &Path, metadata: &Value) -> Result<(), String> {
    fs::write(
        directory.join("run.json"),
        serde_json::to_vec_pretty(metadata).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}
