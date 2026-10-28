// SPDX-License-Identifier: MPL-2.0

use std::{
    env,
    ffi::OsString,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

#[derive(Clone, Copy)]
enum Target {
    Riscv64,
    LoongArch64,
}

impl Target {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "riscv64" => Ok(Self::Riscv64),
            "loongarch64" => Ok(Self::LoongArch64),
            _ => Err(format!("unsupported target: {value}")),
        }
    }

    fn triple(self) -> &'static str {
        match self {
            Self::Riscv64 => "riscv64gc-unknown-none-elf",
            Self::LoongArch64 => "loongarch64-unknown-none",
        }
    }
}

fn main() -> ExitCode {
    match run(env::args_os().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xtask: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: impl IntoIterator<Item = OsString>) -> Result<(), String> {
    let mut arguments = arguments.into_iter();
    let Some(command) = arguments.next() else {
        return Err(usage());
    };
    let command = command
        .into_string()
        .map_err(|_| "command must be valid UTF-8".to_owned())?;

    match command.as_str() {
        "build" => build(arguments),
        "package-init" => package_init(arguments),
        "help" | "--help" | "-h" => {
            println!("{}", usage());
            Ok(())
        }
        _ => Err(usage()),
    }
}

fn build(arguments: impl Iterator<Item = OsString>) -> Result<(), String> {
    let options = BuildOptions::parse(arguments)?;
    run_cargo_build(options.target, options.release)
}

fn package_init(arguments: impl Iterator<Item = OsString>) -> Result<(), String> {
    let options = BuildOptions::parse(arguments)?;
    build_init(options.target, options.release)?;

    let root = workspace_root()?;
    let profile = if options.release { "release" } else { "debug" };
    let target_directory = target_directory(&root);
    let executable = target_directory
        .join(options.target.triple())
        .join(profile)
        .join("init");
    let archive = target_directory
        .join(options.target.triple())
        .join(profile)
        .join("initramfs.cpio");
    let image = fs::read(&executable)
        .map_err(|error| format!("failed to read {}: {error}", executable.display()))?;
    let mut bytes = Vec::new();
    append_entry(&mut bytes, "bin/init", 0o100755, &image)?;
    append_entry(&mut bytes, "TRAILER!!!", 0, &[])?;
    fs::write(&archive, bytes)
        .map_err(|error| format!("failed to write {}: {error}", archive.display()))?;
    println!("{}", archive.display());
    Ok(())
}

struct BuildOptions {
    target: Target,
    release: bool,
}

impl BuildOptions {
    fn parse(arguments: impl Iterator<Item = OsString>) -> Result<Self, String> {
        let mut target = None;
        let mut release = false;
        for argument in arguments {
            let argument = argument
                .into_string()
                .map_err(|_| "arguments must be valid UTF-8".to_owned())?;
            match argument.as_str() {
                "--release" if !release => release = true,
                "riscv64" | "loongarch64" if target.is_none() => {
                    target = Some(Target::parse(&argument)?);
                }
                _ => return Err(usage()),
            }
        }
        Ok(Self {
            target: target.ok_or_else(usage)?,
            release,
        })
    }
}

fn run_cargo_build(target: Target, release: bool) -> Result<(), String> {
    let root = workspace_root()?;
    let mut command = cargo_command(root);
    command.args([
        "build",
        "--locked",
        "-p",
        "arona-arch",
        "--features",
        "baremetal",
        "--target",
        target.triple(),
        "--bin",
        "arch",
    ]);
    if release {
        command.arg("--release");
    }
    execute(command)
}

fn build_init(target: Target, release: bool) -> Result<(), String> {
    let root = workspace_root()?;
    let mut command = cargo_command(root);
    command.args([
        "+nightly",
        "rustc",
        "--locked",
        "-p",
        "arona-init",
        "--features",
        "baremetal",
        "--target",
        target.triple(),
        "--bin",
        "init",
    ]);
    if release {
        command.arg("--release");
    }
    command.args(["--", "-C", "strip=debuginfo"]);
    execute(command)
}

fn cargo_command(root: PathBuf) -> Command {
    let mut command = Command::new("cargo");
    command.current_dir(root);
    command
}

fn workspace_root() -> Result<PathBuf, String> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "xtask must be a workspace member".to_owned())
}

fn target_directory(root: &Path) -> PathBuf {
    match env::var_os("CARGO_TARGET_DIR").map(PathBuf::from) {
        Some(path) if path.is_absolute() => path,
        Some(path) => root.join(path),
        None => root.join("target"),
    }
}

// REFERENCE: https://www.kernel.org/doc/html/latest/driver-api/early-userspace/buffer-format.html
fn append_entry(archive: &mut Vec<u8>, name: &str, mode: u32, data: &[u8]) -> Result<(), String> {
    let name_size = u32::try_from(name.len() + 1).map_err(|_| "CPIO name is too long")?;
    let file_size = u32::try_from(data.len()).map_err(|_| "CPIO file is too large")?;
    archive.extend_from_slice(b"070701");
    for field in [1, mode, 0, 0, 1, 0, file_size, 0, 0, 0, 0, name_size, 0] {
        write!(archive, "{field:08x}").map_err(|error| error.to_string())?;
    }
    archive.extend_from_slice(name.as_bytes());
    archive.push(0);
    pad_to_four(archive);
    archive.extend_from_slice(data);
    pad_to_four(archive);
    Ok(())
}

fn pad_to_four(bytes: &mut Vec<u8>) {
    bytes.resize(bytes.len().next_multiple_of(4), 0);
}

fn execute(mut command: Command) -> Result<(), String> {
    let status = command
        .status()
        .map_err(|error| format!("failed to start cargo: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("cargo exited with {status}"))
    }
}

fn usage() -> String {
    "usage: cargo xtask <build|package-init> <riscv64|loongarch64> [--release]".to_owned()
}
