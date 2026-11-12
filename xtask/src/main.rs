// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

// TODO: Integrate compatibility verification for real Linux applications such as libc and coreutils.
// TODO: Integrate performance benchmarks for IPC, scheduling, file I/O, and networking, along with Linux comparisons.

mod linux_test;
mod net_test;
mod rootfs;

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
        "book" => book(arguments),
        "package-init" => package_init(&BuildOptions::parse(arguments)?, None).map(|_| ()),
        "package-net-test" => {
            package_init(&BuildOptions::parse(arguments)?, Some("net")).map(|_| ())
        }
        "package-linux-test" => {
            package_init(&BuildOptions::parse(arguments)?, Some("linux")).map(|_| ())
        }
        "help" | "--help" | "-h" => {
            println!("{}", usage());
            Ok(())
        }
        _ => Err(usage()),
    }
}

fn book(mut arguments: impl Iterator<Item = OsString>) -> Result<(), String> {
    let action = arguments.next().unwrap_or_else(|| "build".into());
    if !matches!(action.to_str(), Some("build" | "serve")) {
        return Err(usage());
    }
    let mut command = Command::new("mdbook");
    command
        .current_dir(workspace_root()?)
        .arg(action)
        .arg("book")
        .args(arguments);
    execute(command)
}

fn build(arguments: impl Iterator<Item = OsString>, run: bool) -> Result<(), String> {
    let options = BuildOptions::parse(arguments)?;
    let archive = package_init(&options, None)?;
    run_cargo_build(&options, &archive)?;
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
        command.args(["-device", "virtio-rng-pci,disable-legacy=on"]);
        if options.net {
            command.args([
                "-netdev",
                "user,id=net0",
                "-device",
                "virtio-net-pci,disable-legacy=on,netdev=net0,mac=52:54:00:12:34:56",
            ]);
        }
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

fn package_init(options: &BuildOptions, test: Option<&str>) -> Result<PathBuf, String> {
    let root = workspace_root()?;
    let profile = options.profile();
    let target_directory = target_directory(&root);
    let archive = target_directory
        .join(options.target.triple())
        .join(profile)
        .join(test.map_or_else(
            || "initramfs.cpio".to_owned(),
            |test| format!("initramfs-{test}-test.cpio"),
        ));
    let mut bytes = Vec::new();
    for component in ["init", "devmgr", "logger", "fs", "linux", "net"] {
        if test.is_some() && component == "init" {
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
    if test == Some("linux") {
        linux_test::append(options, &root, &target_directory, &mut bytes)?;
    }
    if test == Some("net") {
        net_test::append(options, &root, &mut bytes)?;
    }
    let mut reserved = vec![
        "bin/init",
        "bin/devmgr",
        "bin/logger",
        "bin/fs",
        "bin/linux",
        "bin/net",
    ];
    if test == Some("linux") {
        reserved.extend([
            "bin/linux-test",
            "bin/linux-noexec",
            "bin/linux-compact",
            "bin/linux-separate-headers",
            "bin/linux-truncated",
            "bin/linux-foreign",
            "bin/linux-writable-code",
            "bin/linux-pie",
            "bin/linux-interp",
            "bin/linux-dynamic",
            "etc/linux-data",
        ]);
    }
    if test == Some("net") {
        reserved.push("bin/net-native");
    }
    rootfs::append(&mut bytes, options.rootfs.as_deref(), &reserved)?;
    append_entry(&mut bytes, "TRAILER!!!", 0, &[])?;
    fs::write(&archive, bytes)
        .map_err(|error| format!("failed to write {}: {error}", archive.display()))?;
    println!("{}", archive.display());
    Ok(archive)
}

struct BuildOptions {
    target: Target,
    release: bool,
    initramfs: Initramfs,
    rootfs: Option<PathBuf>,
    net: bool,
}

impl BuildOptions {
    fn parse(mut arguments: impl Iterator<Item = OsString>) -> Result<Self, String> {
        let mut target = None;
        let mut release = false;
        let mut initramfs = Initramfs::Embedded;
        let mut rootfs = None;
        let mut net = false;
        while let Some(argument) = arguments.next() {
            let argument = argument
                .into_string()
                .map_err(|_| "arguments must be valid UTF-8".to_owned())?;
            match argument.as_str() {
                "--release" if !release => release = true,
                "--net" if !net => net = true,
                "--rootfs" if rootfs.is_none() => {
                    rootfs = Some(PathBuf::from(arguments.next().ok_or_else(usage)?));
                }
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
            rootfs,
            net,
        })
    }

    fn profile(&self) -> &'static str {
        if self.release { "release" } else { "debug" }
    }
}

fn run_cargo_build(options: &BuildOptions, archive: &Path) -> Result<(), String> {
    let root = workspace_root()?;
    let mut command = cargo_command(root);
    build_std(&mut command);
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
    build_std(&mut command);
    command.args([
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

/// Rebuilds std so target codegen settings also apply to it.
fn build_std(command: &mut Command) {
    command.args([
        "-Zbuild-std=core,alloc,compiler_builtins",
        "-Zbuild-std-features=compiler-builtins-mem",
    ]);
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
    let inode = u32::try_from(archive.len()).map_err(|_| "CPIO archive is too large")?;
    archive.extend_from_slice(b"070701");
    for field in [inode, mode, 0, 0, 1, 0, file_size, 0, 0, 0, 0, name_size, 0] {
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
    "usage: cargo xtask <build|run|package-init|package-linux-test|package-net-test> <riscv64|loongarch64> [--release] [--initramfs embedded|external] [--rootfs PATH] [--net]\n       cargo xtask book [build|serve] [OPTIONS]".to_owned()
}
