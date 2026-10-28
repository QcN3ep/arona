// SPDX-License-Identifier: MPL-2.0

use std::{
    env,
    ffi::OsString,
    path::Path,
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
        "help" | "--help" | "-h" => {
            println!("{}", usage());
            Ok(())
        }
        _ => Err(usage()),
    }
}

fn build(arguments: impl Iterator<Item = OsString>) -> Result<(), String> {
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
    let target = target.ok_or_else(usage)?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| "xtask must be a workspace member".to_owned())?;
    let mut command = Command::new("cargo");
    command.current_dir(root).args([
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
    "usage: cargo xtask build <riscv64|loongarch64> [--release]".to_owned()
}
