// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Preserves calibration images and verifies that saved symbols describe the running code.

use super::{
    Options, Target, calibration_command, command_output, execute, linux_test, qemu_command,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, env, fs, path::Path, process::Command};

const INPUTS: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    ".cargo",
    "abi",
    "arch",
    "kernel",
    "kernel-tests",
    "components/runtime",
    "xtask",
];

pub(super) fn prepare(options: &Options, root: &Path, run: &Path) -> Result<Value, String> {
    let directory = run.join("artifacts");
    fs::create_dir(&directory).map_err(|error| error.to_string())?;
    let manifest = if let Some(source) = &options.artifacts {
        let source = source.join("artifacts");
        let bytes = fs::read(source.join("manifest.json"))
            .map_err(|error| format!("cannot read saved artifacts: {error}"))?;
        let manifest: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        validate_manifest(&manifest, options, root)?;
        for name in filenames(options.target) {
            verify_file(&source, name, &manifest)?;
            fs::copy(source.join(name), directory.join(name)).map_err(|error| error.to_string())?;
        }
        manifest
    } else {
        create(options, root, &directory)?
    };
    for name in filenames(options.target) {
        verify_file(&directory, name, &manifest)?;
    }
    validate_images(options.target, &directory)?;
    fs::write(
        directory.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    Ok(manifest)
}

fn filenames(target: Target) -> &'static [&'static str] {
    if matches!(target, Target::Riscv64) {
        &["symbols.elf", "guest.elf", "firmware.bin"]
    } else {
        &["symbols.elf", "guest.elf"]
    }
}

fn validate_manifest(manifest: &Value, options: &Options, root: &Path) -> Result<(), String> {
    if manifest["schema"] != 1 || manifest["case"] != "calibration" {
        return Err("unsupported performance artifact format or case".to_owned());
    }
    if manifest["target"] != options.target.triple()
        || manifest["release"].as_bool().is_none()
        || options
            .release
            .is_some_and(|release| manifest["release"] != release)
    {
        return Err("saved artifacts do not match the requested target or profile".to_owned());
    }
    if manifest["machine_arguments"] != json!(machine_arguments(options.target, root)) {
        return Err("saved artifacts use an unsupported QEMU machine configuration".to_owned());
    }
    if manifest["files"].as_object().map(|files| files.len())
        != Some(filenames(options.target).len())
    {
        return Err("unexpected performance artifact files".to_owned());
    }
    Ok(())
}

fn verify_file(directory: &Path, name: &str, manifest: &Value) -> Result<(), String> {
    let path = directory.join(name);
    if manifest["files"][name].as_str() != Some(digest(&path)?.as_str()) {
        return Err(format!("artifact checksum mismatch: {}", path.display()));
    }
    Ok(())
}

pub(super) fn digest(path: &Path) -> Result<String, String> {
    let output = command_output(Command::new("sha256sum").arg("--").arg(path))?;
    let hash = output
        .trim_start_matches('\\')
        .get(..64)
        .ok_or("missing SHA-256 digest")?;
    if !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("invalid SHA-256 digest".to_owned());
    }
    Ok(hash.to_owned())
}

fn create(options: &Options, root: &Path, directory: &Path) -> Result<Value, String> {
    let source = source_state(root)?;
    let command = calibration_command(options, root);
    let arguments = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let environment = command
        .get_envs()
        .map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                value.map(|value| value.to_string_lossy().into_owned()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let executable = linux_test::build_runner(command, "perf")?;
    fs::copy(&executable, directory.join("symbols.elf")).map_err(|error| error.to_string())?;
    if source_state(root)? != source {
        return Err("source inputs changed during the performance build".to_owned());
    }
    let mut strip = Command::new("llvm-objcopy");
    strip
        .arg("--strip-all")
        .arg(directory.join("symbols.elf"))
        .arg(directory.join("guest.elf"));
    execute(strip)?;
    if matches!(options.target, Target::Riscv64) {
        preserve_firmware(options.target, root, directory)?;
    }
    let mut files = BTreeMap::new();
    for name in filenames(options.target) {
        files.insert(*name, digest(&directory.join(name))?);
    }
    Ok(json!({
        "schema": 1, "target": options.target.triple(), "case": "calibration",
        "machine_arguments": machine_arguments(options.target, root),
        "release": options.release.unwrap_or(false), "files": files,
        "build": {
            "source": source, "environment": build_environment(),
            "command": "cargo", "arguments": arguments, "environment_overrides": environment,
            "debug": 2, "strip": "none", "original_elf": executable,
            "rustc": command_output(Command::new("rustc").current_dir(root).arg("-vV"))?,
            "cargo": command_output(Command::new("cargo").current_dir(root).arg("-V"))?,
            "llvm_readobj": command_output(Command::new("llvm-readobj").arg("--version"))?,
            "llvm_objcopy": command_output(Command::new("llvm-objcopy").arg("--version"))?,
            "llvm_nm": command_output(Command::new("llvm-nm").arg("--version"))?,
        },
    }))
}

fn machine_arguments(target: Target, root: &Path) -> Vec<String> {
    qemu_command(target, root)
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

fn source_state(root: &Path) -> Result<Value, String> {
    let git =
        |arguments: &[&str]| command_output(Command::new("git").current_dir(root).args(arguments));
    let commit = git(&["rev-parse", "HEAD"])?;
    let diff = command_output(
        Command::new("git")
            .current_dir(root)
            .args([
                "diff",
                "--binary",
                "--no-ext-diff",
                "--no-textconv",
                "HEAD",
                "--",
            ])
            .args(INPUTS),
    )?;
    let names = command_output(
        Command::new("git")
            .current_dir(root)
            .args(["ls-files", "--others", "--exclude-standard", "-z", "--"])
            .args(INPUTS),
    )?;
    let mut untracked = BTreeMap::new();
    for name in names.split('\0').filter(|name| !name.is_empty()) {
        untracked.insert(name, digest(&root.join(name))?);
    }
    Ok(json!({"commit": commit.trim(), "diff": diff, "untracked": untracked}))
}

fn build_environment() -> BTreeMap<String, String> {
    env::vars()
        .filter(|(name, _)| {
            matches!(
                name.as_str(),
                "RUSTFLAGS"
                    | "CARGO_ENCODED_RUSTFLAGS"
                    | "RUSTC"
                    | "RUSTC_WRAPPER"
                    | "RUSTC_WORKSPACE_WRAPPER"
                    | "RUSTUP_TOOLCHAIN"
                    | "CARGO_TARGET_DIR"
                    | "ARONA_CC"
                    | "ARONA_AR"
            ) || name.starts_with("CARGO_PROFILE_")
                || name.starts_with("CARGO_TARGET_")
        })
        .collect()
}

fn preserve_firmware(target: Target, root: &Path, directory: &Path) -> Result<(), String> {
    let qemu = qemu_command(target, root);
    let paths = command_output(Command::new(qemu.get_program()).args(["-L", "help"]))?;
    let firmware = paths
        .lines()
        .map(|path| Path::new(path).join("opensbi-riscv64-generic-fw_dynamic.bin"))
        .find(|path| path.is_file())
        .ok_or("cannot locate QEMU's RISC-V OpenSBI firmware")?;
    fs::copy(firmware, directory.join("firmware.bin")).map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) fn inspect(path: &Path) -> Result<Value, String> {
    let output = command_output(
        Command::new("llvm-readobj")
            .args([
                "--elf-output-style=JSON",
                "--file-headers",
                "--program-headers",
                "--sections",
            ])
            .arg(path),
    )?;
    let mut files: Vec<Value> = serde_json::from_str(&output).map_err(|error| error.to_string())?;
    if files.len() != 1 {
        return Err("expected one performance ELF image".to_owned());
    }
    Ok(files.remove(0))
}

fn validate_images(target: Target, directory: &Path) -> Result<(), String> {
    let symbols = inspect(&directory.join("symbols.elf"))?;
    let guest = inspect(&directory.join("guest.elf"))?;
    let machine = if matches!(target, Target::Riscv64) {
        243
    } else {
        258
    };
    for image in [&symbols, &guest] {
        if image["ElfHeader"]["Machine"]["Value"] != machine
            || image["ElfHeader"]["Ident"]["Class"]["Value"] != 2
            || image["ElfHeader"]["Ident"]["DataEncoding"]["Value"] != 1
            || image["ElfHeader"]["Type"] != "Executable (0x2)"
        {
            return Err("invalid performance ELF target or format".to_owned());
        }
    }
    for field in ["Entry", "Flags"] {
        if symbols["ElfHeader"][field] != guest["ElfHeader"][field] {
            return Err(format!(
                "ELF {field} differs between symbols and runtime image"
            ));
        }
    }
    let sections = symbols["Sections"]
        .as_array()
        .ok_or("missing ELF sections")?;
    for name in [".symtab", ".debug_info", ".debug_line"] {
        if !sections.iter().any(|section| {
            section["Section"]["Name"]["Name"] == name
                && section["Section"]["Size"]
                    .as_u64()
                    .is_some_and(|size| size > 0)
        }) {
            return Err(format!("saved symbols lack {name}"));
        }
    }
    let symbol_segments = load_segments(&symbols)?;
    let guest_segments = load_segments(&guest)?;
    if symbol_segments.len() != guest_segments.len() || symbol_segments.is_empty() {
        return Err("ELF load segments differ between symbols and runtime image".to_owned());
    }
    let symbol_bytes =
        fs::read(directory.join("symbols.elf")).map_err(|error| error.to_string())?;
    let guest_bytes = fs::read(directory.join("guest.elf")).map_err(|error| error.to_string())?;
    for (symbols, guest) in symbol_segments.iter().zip(guest_segments) {
        for field in [
            "VirtualAddress",
            "PhysicalAddress",
            "FileSize",
            "MemSize",
            "Flags",
            "Alignment",
        ] {
            if symbols[field].is_null() || symbols[field] != guest[field] {
                return Err(format!(
                    "ELF load segment {field} differs between symbols and runtime image"
                ));
            }
        }
        if segment_bytes(&symbol_bytes, symbols)? != segment_bytes(&guest_bytes, guest)? {
            return Err(
                "ELF load segment bytes differ between symbols and runtime image".to_owned(),
            );
        }
    }
    Ok(())
}

fn load_segments(image: &Value) -> Result<Vec<&Value>, String> {
    Ok(image["ProgramHeaders"]
        .as_array()
        .ok_or("missing ELF program headers")?
        .iter()
        .map(|header| &header["ProgramHeader"])
        .filter(|header| header["Type"]["Value"] == 1)
        .collect())
}

fn segment_bytes<'a>(bytes: &'a [u8], segment: &Value) -> Result<&'a [u8], String> {
    let number = |name| {
        segment[name]
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .ok_or("invalid ELF segment bounds")
    };
    let offset = number("Offset")?;
    let end = offset
        .checked_add(number("FileSize")?)
        .ok_or("ELF segment bounds overflow")?;
    bytes
        .get(offset..end)
        .ok_or_else(|| "ELF segment extends beyond the file".to_owned())
}
