// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Collects and validates instruction counts from the architecture calibration image.

mod artifacts;
mod report;

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

const WINDOWS: [&str; 8] = [
    "empty",
    "straight",
    "loop",
    "call",
    "retry",
    "interrupt",
    "retranslation",
    "source",
];
const PASSED: &str = "Perf calibration passed";
const FAILED: &str = "Perf calibration failed:";

struct Options {
    target: Target,
    release: Option<bool>,
    artifacts: Option<PathBuf>,
    profile: bool,
    timeout: Duration,
}

impl Options {
    fn parse(arguments: impl Iterator<Item = OsString>) -> Result<Self, String> {
        let mut arguments = arguments.map(|arg| {
            arg.into_string()
                .map_err(|_| "perf arguments must be valid UTF-8".to_owned())
        });
        let target = Target::parse(&arguments.next().transpose()?.ok_or_else(super::usage)?)?;
        let mut options = Self {
            target,
            release: None,
            artifacts: None,
            profile: false,
            timeout: Duration::from_secs(30),
        };
        while let Some(argument) = arguments.next().transpose()? {
            match argument.as_str() {
                "--release" => options.release = Some(true),
                "--artifacts" => {
                    options.artifacts = Some(PathBuf::from(
                        arguments
                            .next()
                            .transpose()?
                            .ok_or("missing artifacts directory")?,
                    ));
                }
                "--case" => {
                    if arguments.next().transpose()?.as_deref() != Some("calibration") {
                        return Err("--case currently supports only calibration".to_owned());
                    }
                }
                "--mode" => match arguments.next().transpose()?.as_deref() {
                    Some("count") => options.profile = false,
                    Some("profile") => options.profile = true,
                    _ => return Err("--mode supports count or profile".to_owned()),
                },
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
        Ok(options)
    }
}

struct Window {
    name: &'static str,
    start: u64,
    end: u64,
    expected: Option<u64>,
}

pub(super) fn run(mut arguments: impl Iterator<Item = OsString>) -> Result<(), String> {
    match arguments.next().as_deref().and_then(|value| value.to_str()) {
        Some("report") => return report::run(arguments),
        Some("run") => (),
        _ => return Err(super::usage()),
    }
    let options = Options::parse(arguments)?;
    let root = workspace_root()?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let name = format!("{}-{stamp}-{}", options.target.triple(), std::process::id());
    let directory = target_directory(&root).join("perf").join(&name);
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let mut metadata = json!({
        "schema": 2, "status": "preparing", "target": options.target.triple(),
        "case": "calibration", "mode": if options.profile { "profile" } else { "count" }, "plugin_api": 7,
        "timeout_seconds": options.timeout.as_secs(),
    });
    save_metadata(&directory, &metadata)?;
    println!("Performance results: {}", directory.display());
    let outcome = collect(&options, &root, &directory, stamp, &mut metadata);
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
    let windows = read_windows(&saved.join("symbols.elf"))?;
    let plugin = build_plugin(root, directory)?;
    let mut command = qemu_command(options.target, root);
    let version = command_output(Command::new(command.get_program()).arg("--version"))?;
    let control = env::temp_dir().join(format!("arona-perf-{stamp}-{}", std::process::id()));
    if matches!(options.target, Target::Riscv64) {
        command.arg("-bios").arg(saved.join("firmware.bin"));
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
    measure(command, control, directory, options.timeout)?;
    validate_counts(directory, &windows, options.profile)?;
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

fn read_windows(executable: &Path) -> Result<Vec<Window>, String> {
    let output = command_output(
        Command::new("llvm-nm")
            .args(["--format=posix", "--defined-only", "--no-demangle"])
            .arg(executable),
    )?;
    let mut symbols = BTreeMap::new();
    for line in output.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() >= 3 && fields[0].starts_with("__arona_perf_") {
            symbols.insert(
                fields[0],
                u64::from_str_radix(fields[2], 16).map_err(|error| error.to_string())?,
            );
        }
    }
    let symbol = |name, suffix| {
        symbols
            .get(format!("__arona_perf_{name}_{suffix}").as_str())
            .copied()
            .ok_or_else(|| format!("missing calibration symbol: {name}_{suffix}"))
    };
    WINDOWS
        .into_iter()
        .filter(|name| *name != "source" || symbols.contains_key("__arona_perf_source_start"))
        .map(|name| {
            Ok(Window {
                name,
                start: symbol(name, "start")?,
                end: symbol(name, "end")?,
                expected: if name == "source" {
                    None
                } else {
                    Some(symbol(name, "expected")?)
                },
            })
        })
        .collect()
}

fn plugin_options(
    plugin: &Path,
    directory: &Path,
    windows: &[Window],
    profile: bool,
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
            ",window={}:{:x}:{:x}",
            window.name, window.start, window.end
        )
        .map_err(|error| error.to_string())?;
    }
    if profile {
        options.push_str(",profile=static-calibration");
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
) -> Result<(), String> {
    let serial = directory.join("serial.log");
    command
        .stdin(Stdio::null())
        .stdout(File::create(&serial).map_err(|error| error.to_string())?)
        .stderr(File::create(directory.join("qemu.log")).map_err(|error| error.to_string())?);
    fs::create_dir(&control).map_err(|error| error.to_string())?;
    let child = command.spawn().map_err(|error| {
        let _ = fs::remove_dir(&control);
        error.to_string()
    })?;
    let mut emulator = Emulator { child, control };
    let deadline = Instant::now() + timeout;
    loop {
        let bytes = fs::read(&serial).map_err(|error| error.to_string())?;
        let log = String::from_utf8_lossy(&bytes);
        let failed = log.contains(FAILED);
        let passed = log.lines().any(|line| line == PASSED);
        if let Some(status) = emulator
            .child
            .try_wait()
            .map_err(|error| error.to_string())?
        {
            return if status.success() && passed && !failed {
                Ok(())
            } else {
                Err(format!(
                    "QEMU exited before successful calibration: {status}; see qemu.log"
                ))
            };
        }
        if failed || passed {
            let stopped = emulator.stop();
            return if failed {
                Err("guest calibration failed; see serial.log".to_owned())
            } else {
                stopped
            };
        }
        if Instant::now() >= deadline {
            let _ = emulator.stop();
            return Err(format!(
                "calibration timed out after {} seconds",
                timeout.as_secs()
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn validate_counts(directory: &Path, expected: &[Window], profile: bool) -> Result<(), String> {
    let bytes = fs::read(directory.join("counts.json"))
        .map_err(|error| format!("cannot read instruction counts: {error}; see qemu.log"))?;
    let counts: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    let schema = if profile { 2 } else { 1 };
    if counts["schema"] != schema
        || counts["plugin_api"] != 7
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
        println!("{}: {} instructions", expected.name, window["instructions"]);
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
