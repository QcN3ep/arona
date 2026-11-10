// SPDX-License-Identifier: MPL-2.0

use super::{BuildOptions, Target, append_entry, cargo_command, execute};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub(super) fn append(
    options: BuildOptions,
    root: &Path,
    target_directory: &Path,
    bytes: &mut Vec<u8>,
) -> Result<(), String> {
    let (executable, compact) = build_guest(options, root, target_directory)?;
    let image = fs::read(executable).map_err(|error| error.to_string())?;
    append_entry(bytes, "bin/linux-test", 0o100755, &image)?;
    let mut compact = fs::read(compact).map_err(|error| error.to_string())?;
    append_entry(bytes, "bin/linux-compact", 0o100755, &compact)?;
    let headers = u64::from_le_bytes(compact[32..40].try_into().unwrap()) as usize;
    let count = u16::from_le_bytes(compact[56..58].try_into().unwrap()) as usize;
    for header in compact[headers..headers + count * 56]
        .as_chunks_mut::<56>()
        .0
    {
        if u32::from_le_bytes(header[..4].try_into().unwrap()) == 1 {
            header[..4].copy_from_slice(&0u32.to_le_bytes());
            break;
        }
    }
    append_entry(bytes, "bin/linux-separate-headers", 0o100755, &compact)?;
    append_entry(bytes, "bin/linux-truncated", 0o100755, &image[..63])?;
    let mut foreign = image.clone();
    foreign[18..20].copy_from_slice(&0u16.to_le_bytes());
    append_entry(bytes, "bin/linux-foreign", 0o100755, &foreign)?;
    let mut writable = image;
    let headers = u64::from_le_bytes(writable[32..40].try_into().unwrap()) as usize;
    let count = u16::from_le_bytes(writable[56..58].try_into().unwrap()) as usize;
    for header in writable[headers..headers + count * 56]
        .as_chunks_mut::<56>()
        .0
    {
        if u32::from_le_bytes(header[..4].try_into().unwrap()) == 1 {
            header[4..8].copy_from_slice(&7u32.to_le_bytes());
            break;
        }
    }
    append_entry(bytes, "bin/linux-writable-code", 0o100755, &writable)?;
    let mut data = b"0123456789abcdef".repeat(512);
    data.push(b'Z');
    append_entry(bytes, "etc/linux-data", 0o100644, &data)?;
    let mut command = cargo_command(root.to_path_buf());
    command.args([
        "+nightly",
        "rustc",
        "--locked",
        "-p",
        "arona-linux",
        "--features",
        "baremetal",
        "--test",
        "program",
        "--target",
        options.target.triple(),
        "--message-format=json",
    ]);
    if options.release {
        command.arg("--release");
    }
    command.args(["--", "-Cstrip=symbols", "-Clink-arg=-zmax-page-size=4096"]);
    let executable = build_runner(command)?;
    append_entry(
        bytes,
        "bin/init",
        0o100755,
        &fs::read(executable).map_err(|error| error.to_string())?,
    )?;

    Ok(())
}

fn build_runner(mut command: Command) -> Result<PathBuf, String> {
    let output = command.output().map_err(|error| error.to_string())?;
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    let mut executable = None;
    for line in output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let item: serde_json::Value =
            serde_json::from_slice(line).map_err(|error| error.to_string())?;
        if item["reason"] == "compiler-message"
            && let Some(rendered) = item["message"]["rendered"].as_str()
        {
            eprint!("{rendered}");
        }
        if item["reason"] == "compiler-artifact" && item["target"]["name"] == "program" {
            executable = item["executable"].as_str().map(PathBuf::from);
        }
    }
    if !output.status.success() {
        return Err(format!("Linux test build failed: {}", output.status));
    }
    executable.ok_or_else(|| "missing Linux test executable".to_owned())
}

fn build_guest(
    options: BuildOptions,
    root: &Path,
    target: &Path,
) -> Result<(PathBuf, PathBuf), String> {
    let (isa, triple) = match options.target {
        Target::Riscv64 => ("riscv64", "riscv64-unknown-linux-gnu"),
        Target::LoongArch64 => ("loongarch64", "loongarch64-unknown-linux-gnu"),
    };
    let directory = target
        .join(options.target.triple())
        .join(options.profile())
        .join("linux-tests");
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let mut objects = Vec::new();
    for (module, name) in [
        ("program", "program"),
        ("memory", "memory"),
        ("syscall/file", "file"),
        ("syscall/file/io", "file-io"),
        ("syscall/file/buffer", "file-buffer"),
        ("syscall/file/metadata", "file-metadata"),
        ("syscall/file/directory", "file-directory"),
        ("syscall/file/path", "file-path"),
        ("syscall/file/pipe", "file-pipe"),
        ("syscall/file/poll", "file-poll"),
        ("syscall/memory", "file-mappings"),
    ] {
        let object = directory.join(format!("{name}.o"));
        let mut assemble = Command::new("clang");
        assemble
            .arg(format!("--target={triple}"))
            .args(["-c", "-x", "assembler"])
            .arg(root.join(format!("components/linux/tests/{module}/{isa}.S")))
            .arg("-o")
            .arg(&object);
        execute(assemble)?;
        objects.push(object);
    }
    for (name, address) in [("linux-test", "0x400000"), ("linux-compact", "0x11000")] {
        let mut link = Command::new("ld.lld");
        link.args(["-static", "--entry=_start", "-z", "max-page-size=4096"])
            .arg(format!("-Ttext={address}"))
            .args(&objects)
            .arg("-o")
            .arg(directory.join(name));
        execute(link)?;
    }
    Ok((
        directory.join("linux-test"),
        directory.join("linux-compact"),
    ))
}
