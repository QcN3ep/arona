// SPDX-License-Identifier: MPL-2.0

mod linux_test;

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

#[derive(Clone, Copy)]
enum Initramfs {
    Embedded,
    External,
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
        "build" => build(arguments, false),
        "run" => build(arguments, true),
        "package-init" => package_init(BuildOptions::parse(arguments)?, false).map(|_| ()),
        "package-linux-test" => package_init(BuildOptions::parse(arguments)?, true).map(|_| ()),
        "help" | "--help" | "-h" => {
            println!("{}", usage());
            Ok(())
        }
        _ => Err(usage()),
    }
}

fn build(arguments: impl Iterator<Item = OsString>, run: bool) -> Result<(), String> {
    let options = BuildOptions::parse(arguments)?;
    let archive = package_init(options, false)?;
    run_cargo_build(options, &archive)?;
    if run {
        let root = workspace_root()?;
        let isa = match options.target {
            Target::Riscv64 => "riscv64",
            Target::LoongArch64 => "loongarch64",
        };
        let mut command = Command::new(format!("qemu-system-{isa}"));
        command
            .current_dir(&root)
            .args([
                "-M",
                "virt",
                "-m",
                "256M",
                "-smp",
                "1",
                "-nographic",
                "-monitor",
                "none",
                "-nic",
                "none",
                "-kernel",
            ])
            .arg(
                target_directory(&root)
                    .join(options.target.triple())
                    .join(options.profile())
                    .join("arch"),
            );
        if matches!(options.target, Target::Riscv64) {
            command.args(["-bios", "default"]);
        }
        if matches!(options.initramfs, Initramfs::External) {
            command.arg("-initrd").arg(archive);
        }
        execute(command)?;
    }
    Ok(())
}

fn package_init(options: BuildOptions, linux_test: bool) -> Result<PathBuf, String> {
    let root = workspace_root()?;
    let profile = options.profile();
    let target_directory = target_directory(&root);
    let archive = target_directory
        .join(options.target.triple())
        .join(profile)
        .join(if linux_test {
            "initramfs-linux-test.cpio"
        } else {
            "initramfs.cpio"
        });
    let mut bytes = Vec::new();
    for component in ["init", "devmgr", "logger", "fs", "linux"] {
        if linux_test && component == "init" {
            continue;
        }
        build_component(options.target, options.release, component)?;
        let executable = target_directory
            .join(options.target.triple())
            .join(profile)
            .join(component);
        let image = fs::read(&executable)
            .map_err(|error| format!("failed to read {}: {error}", executable.display()))?;
        append_entry(&mut bytes, &format!("bin/{component}"), 0o100755, &image)?;
    }
    if linux_test {
        linux_test::append(options, &root, &target_directory, &mut bytes)?;
    }
    append_entry(&mut bytes, "TRAILER!!!", 0, &[])?;
    fs::write(&archive, bytes)
        .map_err(|error| format!("failed to write {}: {error}", archive.display()))?;
    println!("{}", archive.display());
    Ok(archive)
}

#[derive(Clone, Copy)]
struct BuildOptions {
    target: Target,
    release: bool,
    initramfs: Initramfs,
}

impl BuildOptions {
    fn parse(mut arguments: impl Iterator<Item = OsString>) -> Result<Self, String> {
        let mut target = None;
        let mut release = false;
        let mut initramfs = Initramfs::Embedded;
        while let Some(argument) = arguments.next() {
            let argument = argument
                .into_string()
                .map_err(|_| "arguments must be valid UTF-8".to_owned())?;
            match argument.as_str() {
                "--release" if !release => release = true,
                "--initramfs" => {
                    initramfs = match arguments.next().as_deref().and_then(|value| value.to_str()) {
                        Some("embedded") => Initramfs::Embedded,
                        Some("external") => Initramfs::External,
                        _ => return Err(usage()),
                    };
                }
                "riscv64" | "loongarch64" if target.is_none() => {
                    target = Some(Target::parse(&argument)?);
                }
                _ => return Err(usage()),
            }
        }
        Ok(Self {
            target: target.ok_or_else(usage)?,
            release,
            initramfs,
        })
    }

    fn profile(self) -> &'static str {
        if self.release { "release" } else { "debug" }
    }
}

fn run_cargo_build(options: BuildOptions, archive: &Path) -> Result<(), String> {
    let root = workspace_root()?;
    let mut command = cargo_command(root);
    command.args([
        "build",
        "--locked",
        "-p",
        "arona-arch",
        "--features",
        if matches!(options.initramfs, Initramfs::Embedded) {
            "baremetal,embedded-initramfs"
        } else {
            "baremetal"
        },
        "--target",
        options.target.triple(),
        "--bin",
        "arch",
    ]);
    command.env("ARONA_INITRAMFS", archive);
    if options.release {
        command.arg("--release");
    }
    execute(command)
}

fn build_component(target: Target, release: bool, component: &str) -> Result<(), String> {
    let root = workspace_root()?;
    let mut command = cargo_command(root);
    command.args([
        "+nightly",
        "rustc",
        "--locked",
        "-p",
        &format!("arona-{component}"),
        "--features",
        "baremetal",
        "--target",
        target.triple(),
        "--bin",
        component,
    ]);
    if release {
        command.arg("--release");
    }
    command.args([
        "--",
        "-C",
        "strip=symbols",
        "-C",
        "link-arg=-zmax-page-size=4096",
    ]);
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
    let status = command.status().map_err(|error| {
        format!(
            "failed to start {}: {error}",
            command.get_program().to_string_lossy()
        )
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} exited with {status}",
            command.get_program().to_string_lossy()
        ))
    }
}

fn usage() -> String {
    "usage: cargo xtask <build|run|package-init|package-linux-test> <riscv64|loongarch64> [--release] [--initramfs embedded|external]".to_owned()
}
