// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::{BuildOptions, Target, append_entry, build_std, cargo_command, execute};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub(super) fn append(
    options: &BuildOptions,
    root: &Path,
    target_directory: &Path,
    bytes: &mut Vec<u8>,
) -> Result<(), String> {
    let (executable, compact) = build_guest(options, root, target_directory)?;
    let image = fs::read(executable).map_err(|error| error.to_string())?;
    append_entry(bytes, "bin/linux-test", 0o100755, &image)?;
    append_entry(
        bytes,
        "bin/linux-socket",
        0o100755,
        &socket_guest(options, root, target_directory)?,
    )?;
    append_entry(bytes, "bin/linux-noexec", 0o100644, &image)?;
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
    for (name, image) in fixtures(options, root, target_directory)? {
        append_entry(bytes, name, 0o100755, &image)?;
    }
    for (name, script) in [
        (
            "bin/linux-script",
            b"#! \t/bin/linux-pie \toption with space \t\n".as_slice(),
        ),
        ("bin/linux-empty-script", b"#! \t\n"),
        ("bin/linux-script-loop", b"#!/bin/linux-script-loop\n"),
    ] {
        append_entry(bytes, name, 0o100755, script)?;
    }
    let mut data = b"0123456789abcdef".repeat(512);
    data.push(b'Z');
    append_entry(bytes, "etc/linux-data", 0o100644, &data)?;
    let mut command = cargo_command(root.to_path_buf());
    build_std(&mut command);
    command.args([
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
    let executable = build_runner(command, "program")?;
    append_entry(
        bytes,
        "bin/init",
        0o100755,
        &fs::read(executable).map_err(|error| error.to_string())?,
    )?;

    Ok(())
}

pub(super) fn build_runner(mut command: Command, name: &str) -> Result<PathBuf, String> {
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
        if item["reason"] == "compiler-artifact" && item["target"]["name"] == name {
            executable = item["executable"].as_str().map(PathBuf::from);
        }
    }
    if !output.status.success() {
        return Err(format!("{name} test build failed: {}", output.status));
    }
    executable.ok_or_else(|| format!("missing {name} test executable"))
}

fn build_guest(
    options: &BuildOptions,
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
        ("process/terminal", "process-terminal"),
        ("syscall/system", "system"),
        ("syscall/signal", "signal"),
        ("syscall/process", "process"),
        ("syscall/process/futex", "futex"),
        ("syscall/process/child", "child"),
        ("syscall/process/identity", "identity"),
        ("syscall/process/exec", "exec"),
        ("memory", "memory"),
        ("syscall/file", "file"),
        ("syscall/file/io", "file-io"),
        ("syscall/file/buffer", "file-buffer"),
        ("syscall/file/metadata", "file-metadata"),
        ("syscall/file/directory", "file-directory"),
        ("syscall/file/path", "file-path"),
        ("syscall/file/pipe", "file-pipe"),
        ("syscall/file/poll", "file-poll"),
        ("syscall/file/terminal", "file-terminal"),
        ("syscall/file/virtual", "file-virtual"),
        ("syscall/memory", "file-mappings"),
    ] {
        let object = directory.join(format!("{name}.o"));
        let mut assemble = Command::new("clang");
        assemble
            .arg(format!("--target={triple}"))
            .args(["-c", "-x", "assembler"]);
        if isa == "riscv64" {
            assemble.arg("-mno-relax");
        }
        assemble
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

fn socket_guest(options: &BuildOptions, root: &Path, target: &Path) -> Result<Vec<u8>, String> {
    let (isa, triple) = match options.target {
        Target::Riscv64 => ("riscv64", "riscv64-unknown-linux-gnu"),
        Target::LoongArch64 => ("loongarch64", "loongarch64-unknown-linux-gnu"),
    };
    let directory = target
        .join(options.target.triple())
        .join(options.profile())
        .join("linux-tests");
    let mut objects = Vec::new();
    for name in ["stream.c", "datagram.c", "options.c", &format!("{isa}.S")] {
        let object = directory.join(format!("socket-{name}.o"));
        let mut compile = Command::new("clang");
        compile.arg(format!("--target={triple}")).args([
            "-c",
            "-O2",
            "-ffreestanding",
            "-fno-builtin",
            "-fno-stack-protector",
            "-fno-pic",
            "-Wall",
            "-Wextra",
            "-Werror",
        ]);
        if isa == "riscv64" {
            compile.arg("-mno-relax");
        } else {
            compile.args(["-mno-lsx", "-mno-lasx"]);
        }
        compile
            .arg(
                root.join("components/linux/tests/syscall/socket")
                    .join(name),
            )
            .arg("-o")
            .arg(&object);
        execute(compile)?;
        objects.push(object);
    }
    let executable = directory.join("linux-socket");
    let mut link = Command::new("ld.lld");
    link.args([
        "-static",
        "--entry=_start",
        "-z",
        "max-page-size=4096",
        "-Ttext=0x400000",
    ])
    .args(&objects)
    .arg("-o")
    .arg(&executable);
    execute(link)?;
    fs::read(executable).map_err(|error| error.to_string())
}

const FIXTURE_ENTRY: usize = 0x100;
const FIXTURE_PATH: usize = 0x200;
const FIXTURE_INTERPRETER: &[u8] = b"/bin/linux-interp";
/// Illegal instruction bytes; a PT_INTERP image must never enter its main entry.
const UNUSED_CODE: [u8; 4] = [0; 4];

/// Build the PT_INTERP handoff fixtures used by the exec scenarios.
fn fixtures(
    options: &BuildOptions,
    root: &Path,
    target: &Path,
) -> Result<[(&'static str, Vec<u8>); 3], String> {
    let (isa, triple, machine) = match options.target {
        Target::Riscv64 => ("riscv64", "riscv64-unknown-linux-gnu", 243),
        Target::LoongArch64 => ("loongarch64", "loongarch64-unknown-linux-gnu", 258),
    };
    let pie = assemble_fixture(options, root, target, isa, triple, "program/pie")?;
    let interpreter = assemble_fixture(options, root, target, isa, triple, "program/interp")?;
    Ok([
        ("bin/linux-pie", image(machine, &pie, None)),
        ("bin/linux-interp", image(machine, &interpreter, None)),
        (
            "bin/linux-dynamic",
            image(machine, &UNUSED_CODE, Some(FIXTURE_INTERPRETER)),
        ),
    ])
}

fn assemble_fixture(
    options: &BuildOptions,
    root: &Path,
    target: &Path,
    isa: &str,
    triple: &str,
    module: &str,
) -> Result<Vec<u8>, String> {
    let directory = target
        .join(options.target.triple())
        .join(options.profile())
        .join("linux-tests");
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let name = module.replace('/', "-");
    let object = directory.join(format!("{name}.o"));
    let mut assemble = Command::new("clang");
    assemble
        .arg(format!("--target={triple}"))
        .args(["-c", "-x", "assembler"]);
    if isa == "riscv64" {
        assemble.arg("-mno-relax");
    }
    assemble
        .arg(root.join(format!("components/linux/tests/{module}/{isa}.S")))
        .arg("-o")
        .arg(&object);
    execute(assemble)?;
    let binary = directory.join(format!("{name}.bin"));
    let mut link = Command::new("ld.lld");
    link.args(["--oformat=binary", "--image-base=0"])
        .arg(format!("-Ttext={FIXTURE_ENTRY:#x}"))
        .arg(&object)
        .arg("-o")
        .arg(&binary);
    execute(link)?;
    fs::read(binary).map_err(|error| error.to_string())
}

/// Wrap position-independent code in an ET_DYN image with an optional PT_INTERP.
fn image(machine: u16, code: &[u8], interpreter: Option<&[u8]>) -> Vec<u8> {
    let path_size = interpreter.map_or(0, |path| path.len() + 1);
    let dynamic = (FIXTURE_ENTRY + code.len())
        .max(FIXTURE_PATH + path_size)
        .next_multiple_of(16);
    let length = dynamic + 16;
    let mut bytes = vec![0; length];
    // e_ident: ELF64, little endian, version 1.
    bytes[..4].copy_from_slice(b"\x7fELF");
    bytes[4] = 2;
    bytes[5] = 1;
    bytes[6] = 1;
    // e_type is ET_DYN, followed by e_machine, e_version, e_entry, and e_phoff.
    bytes[16..18].copy_from_slice(&3u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&machine.to_le_bytes());
    bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
    bytes[24..32].copy_from_slice(&(FIXTURE_ENTRY as u64).to_le_bytes());
    bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
    bytes[52..54].copy_from_slice(&64u16.to_le_bytes());
    bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
    bytes[56..58].copy_from_slice(&(if interpreter.is_some() { 3u16 } else { 2 }).to_le_bytes());
    program_header(&mut bytes[64..120], 1, 5, 0, length, 4096);
    if let Some(interpreter) = interpreter {
        program_header(&mut bytes[120..176], 3, 4, FIXTURE_PATH, path_size, 1);
        bytes[FIXTURE_PATH..FIXTURE_PATH + interpreter.len()].copy_from_slice(interpreter);
    }
    let header = if interpreter.is_some() { 176 } else { 120 };
    program_header(&mut bytes[header..header + 56], 2, 4, dynamic, 16, 8);
    bytes[FIXTURE_ENTRY..FIXTURE_ENTRY + code.len()].copy_from_slice(code);
    bytes
}

fn program_header(
    output: &mut [u8],
    kind: u32,
    flags: u32,
    offset: usize,
    file_size: usize,
    alignment: usize,
) {
    output[..4].copy_from_slice(&kind.to_le_bytes());
    output[4..8].copy_from_slice(&flags.to_le_bytes());
    output[8..16].copy_from_slice(&(offset as u64).to_le_bytes());
    output[16..24].copy_from_slice(&(offset as u64).to_le_bytes());
    output[24..32].copy_from_slice(&(offset as u64).to_le_bytes());
    output[32..40].copy_from_slice(&(file_size as u64).to_le_bytes());
    output[40..48].copy_from_slice(&(file_size as u64).to_le_bytes());
    output[48..56].copy_from_slice(&(alignment as u64).to_le_bytes());
}
