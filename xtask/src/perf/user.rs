// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Build user performance images for the shared case discovery protocol.

use super::{Options, artifacts, command_output, linux_test};
use crate::{append_entry, build_std, cargo_command, test};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub(super) fn build(
    options: &mut Options,
    root: &Path,
    directory: &Path,
    builds: &mut test::build::Build,
    source: Value,
) -> Result<(PathBuf, Value, Vec<test::Case>), String> {
    let command = builds.program_command(options.case, "kperf");
    let program_build = artifacts::build_description(&command, root, source.clone())?;
    let program = linux_test::build_runner(command, "kperf")?;
    let mut archive = Vec::new();
    append_entry(
        &mut archive,
        "bin/init",
        0o100755,
        &fs::read(&program).map_err(|error| error.to_string())?,
    )?;
    append_entry(&mut archive, "TRAILER!!!", 0, &[])?;
    let initrd = directory.join("initramfs.cpio");
    fs::write(&initrd, archive).map_err(|error| error.to_string())?;
    let mut command = cargo_command(root.to_path_buf());
    build_std(&mut command);
    command.args([
        "build",
        "--locked",
        "-p",
        "arona-arch",
        "--features",
        "baremetal",
        "--target",
        options.target.triple(),
        "--bin",
        "arch",
        "--message-format=json",
    ]);
    if options.release.unwrap_or(false) {
        command.arg("--release");
    }
    let profile = if options.release.unwrap_or(false) {
        "RELEASE"
    } else {
        "DEV"
    };
    command.env(format!("CARGO_PROFILE_{profile}_DEBUG"), "2");
    command.env(format!("CARGO_PROFILE_{profile}_STRIP"), "none");
    let kernel_build = artifacts::build_description(&command, root, source)?;
    let kernel = linux_test::build_runner(command, "arch")?;
    options.input.kernel = Some(kernel.clone());
    options.input.initramfs = Some(initrd.clone());
    options.input.images = vec![format!("bin/init={}", program.display())];
    options.input.start = Some(boundary(&program, "start")?);
    options.input.end = Some(boundary(&program, "end")?);
    let cases = test::vm::list_with_initrd(options.target, root, &kernel, &initrd, directory)?;
    Ok((
        kernel,
        json!({"program":program_build, "kernel":kernel_build}),
        cases,
    ))
}

fn boundary(program: &Path, name: &str) -> Result<String, String> {
    let output = command_output(
        Command::new("llvm-nm")
            .args(["--defined-only", "--format=posix", "--no-demangle"])
            .arg(program),
    )?;
    let expected = format!("testkit_core::user::{name}");
    let symbols: Vec<_> = output
        .lines()
        .filter_map(|line| {
            let symbol = line.split_whitespace().next()?;
            let decoded = rustc_demangle::try_demangle(symbol).ok()?;
            (format!("{decoded:#}") == expected).then_some(symbol)
        })
        .collect();
    if symbols.len() != 1 {
        return Err(format!(
            "expected one user measurement boundary {expected}, found {}",
            symbols.len()
        ));
    }
    Ok(format!("bin/init:{}", symbols[0]))
}

pub(super) fn fixture(
    options: &mut Options,
    root: &Path,
    directory: &Path,
    case: &str,
) -> Result<(), String> {
    if options.case != "fs" {
        return Ok(());
    }
    options.input.disks.clear();
    let backend = case
        .strip_prefix("kperf::")
        .and_then(|name| name.split_once("::"))
        .map(|(backend, _)| backend)
        .ok_or("missing filesystem backend")?;
    if backend == "tmpfs" {
        return Ok(());
    }
    let disk = directory.join(format!("{backend}.raw"));
    if !disk.exists() {
        let mut command = Command::new("python3");
        command
            .arg(root.join("components/fs/tests/kperf/fixture.py"))
            .arg(backend)
            .arg(&disk);
        crate::execute(command)?;
    }
    options.input.disks.push(disk);
    Ok(())
}
