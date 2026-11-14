// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Build and run host tests and isolated QEMU test cases.

mod build;
mod resources;
mod vm;

use super::{Target, cargo_command, execute, target_directory, workspace_root};
use serde_json::json;
use std::{
    ffi::OsString,
    fs,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub(super) struct Options {
    target: Option<Target>,
    release: bool,
    filter: String,
    list: bool,
    timeout: Option<Duration>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Case {
    name: String,
    expected: Vec<String>,
}

impl Options {
    fn parse(mut arguments: impl Iterator<Item = OsString>) -> Result<Self, String> {
        let target = arguments.next().ok_or_else(super::usage)?;
        let target = match target.to_str() {
            Some("host") => None,
            Some(name) => Some(Target::parse(name)?),
            None => return Err("test target must be valid UTF-8".to_owned()),
        };
        let mut options = Self {
            target,
            release: false,
            filter: String::new(),
            list: false,
            timeout: None,
        };
        while let Some(argument) = arguments.next() {
            match argument.to_str() {
                Some("--release") => options.release = true,
                Some("--list") => options.list = true,
                Some("--filter") => {
                    options.filter = arguments
                        .next()
                        .and_then(|value| value.into_string().ok())
                        .ok_or("--filter requires a UTF-8 test name")?
                }
                Some("--timeout") => {
                    let seconds = arguments
                        .next()
                        .and_then(|value| value.into_string().ok())
                        .and_then(|value| value.parse::<u64>().ok())
                        .filter(|seconds| *seconds > 0)
                        .ok_or("--timeout requires a positive number of seconds")?;
                    options.timeout = Some(Duration::from_secs(seconds));
                }
                _ => return Err(super::usage()),
            }
        }
        if options.target.is_none() && options.timeout.is_some() {
            return Err("--timeout applies only to QEMU tests".to_owned());
        }
        Ok(options)
    }
}

pub(super) fn run(arguments: impl Iterator<Item = OsString>) -> Result<(), String> {
    let options = Options::parse(arguments)?;
    let root = workspace_root()?;
    let Some(target) = options.target else {
        let mut command = cargo_command(root);
        command.args([
            "test",
            "--locked",
            "--workspace",
            "--tests",
            "--features",
            "arona-fs/host-tests",
        ]);
        if options.release {
            command.arg("--release");
        }
        if !options.filter.is_empty() {
            command.arg(&options.filter);
        }
        if options.list {
            command.args(["--", "--list"]);
        }
        return execute(command);
    };
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let directory = target_directory(&root)
        .join("tests")
        .join(target.triple())
        .join(if options.release { "release" } else { "debug" })
        .join(format!("{stamp}-{}", std::process::id()));
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    println!("Test results: {}", directory.display());
    let mut builds = build::Build::new(root.clone(), target, options.release);
    let mut results = Vec::new();
    let mut matched = 0;
    for image in [
        "paging",
        "allocator",
        "trap",
        "execution",
        "timer",
        "interrupt",
    ] {
        let kernel = builds.image(image)?;
        let listing = directory.join(format!("list-{image}"));
        fs::create_dir(&listing).map_err(|error| error.to_string())?;
        let cases = vm::list(target, &root, &kernel, &listing)?;
        for case in cases
            .iter()
            .filter(|case| case.name.contains(&options.filter))
        {
            matched += 1;
            if options.list {
                println!("{}", case.name);
                continue;
            }
            let output = directory.join(case.name.replace("::", "-"));
            fs::create_dir(&output).map_err(|error| error.to_string())?;
            println!("test {} ...", case.name);
            let outcome = run_case(&options, &mut builds, &kernel, case, &output);
            println!(
                "test {} ... {}",
                case.name,
                if outcome.is_ok() { "ok" } else { "FAILED" }
            );
            if let Err(error) = &outcome {
                eprintln!("{error}");
            }
            results.push(
                json!({"name": case.name, "passed": outcome.is_ok(), "error": outcome.err()}),
            );
            fs::write(
                directory.join("results.json"),
                serde_json::to_vec_pretty(&results).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
        }
    }
    if matched == 0 {
        return Err(format!("no QEMU tests match {:?}", options.filter));
    }
    let failures = results
        .iter()
        .filter(|result| result["passed"] == false)
        .count();
    if failures != 0 {
        return Err(format!(
            "{failures} of {matched} tests failed; see {}",
            directory.display()
        ));
    }
    if !options.list {
        println!("test result: ok. {matched} passed");
    }
    Ok(())
}

fn run_case(
    options: &Options,
    builds: &mut build::Build,
    kernel: &std::path::Path,
    case: &Case,
    directory: &std::path::Path,
) -> Result<(), String> {
    let mut resources = resources::Resources::prepare(builds, case, directory)?;
    let timeout = options.timeout.unwrap_or_else(|| {
        Duration::from_secs(match case.name.as_str() {
            "interrupt::fs" | "interrupt::linux" => 900,
            "interrupt::init" => 300,
            _ => 120,
        })
    });
    vm::run(
        builds.target,
        &builds.root,
        kernel,
        directory,
        case,
        &mut resources,
        timeout,
    )?;
    resources.finish(builds, kernel, directory, case, timeout)
}
