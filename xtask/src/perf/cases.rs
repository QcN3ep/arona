// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Discover performance cases through the shared test harness and select one per measurement.

use super::{Options, artifacts, execute_run, linux_test};
use crate::{target_directory, test};
use serde_json::Value;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub(super) struct Selection {
    pub name: String,
    pub iterations: u64,
    pub executable: PathBuf,
    pub build: Value,
}

pub(super) fn run(mut options: Options, root: &Path) -> Result<(), String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let directory = target_directory(root)
        .join("perf")
        .join(format!("discovery-{stamp}-{}", std::process::id()));
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let mut builds = test::build::Build::new(
        root.to_path_buf(),
        options.target,
        options.release.unwrap_or(false),
    );
    builds.symbols = true;
    let mut matched = 0;
    let mut failures = 0;
    let source = artifacts::source_state(root)?;
    let command = builds.image_command("kperf")?;
    let build = if options.list {
        Value::Null
    } else {
        artifacts::build_description(&command, root, source.clone())?
    };
    let executable = linux_test::build_runner(command, "kperf")?;
    if artifacts::source_state(root)? != source {
        return Err("source inputs changed during the performance build".to_owned());
    }
    let cases = test::vm::list(options.target, root, &executable, &directory)?;
    for case in cases
        .into_iter()
        .filter(|case| case.performance && case.name.contains(&options.filter))
    {
        matched += 1;
        if options.list {
            println!("{}", case.name);
            continue;
        }
        println!("perf {} ...", case.name);
        options.selection = Some(Selection {
            name: case.name.clone(),
            iterations: options.iterations.unwrap_or(1),
            executable: executable.clone(),
            build: build.clone(),
        });
        if let Err(error) = execute_run(&options, root) {
            failures += 1;
            eprintln!("perf {} ... FAILED: {error}", case.name);
        }
    }
    if matched == 0 {
        return Err(format!("no performance cases match {:?}", options.filter));
    }
    if failures != 0 {
        return Err(format!("{failures} of {matched} performance cases failed"));
    }
    Ok(())
}

pub(super) fn selection(manifest: &Value) -> Result<(&str, u64), String> {
    let measurement = &manifest["measurement"];
    let name = measurement["workload"]
        .as_str()
        .ok_or("missing performance case name")?;
    let iterations = measurement["units"]
        .as_u64()
        .filter(|count| *count > 0)
        .ok_or("missing positive iteration count")?;
    Ok((name, iterations))
}

pub(super) fn select(
    output: &[u8],
    name: &str,
    iterations: u64,
    input: &mut impl Write,
) -> Result<(), String> {
    if !test::vm::parse_cases(output)?
        .iter()
        .any(|case| case.performance && case.name == name)
    {
        return Err(format!(
            "test image does not declare performance case {name}"
        ));
    }
    writeln!(input, "{name}\t{iterations}")
        .and_then(|()| input.flush())
        .map_err(|error| error.to_string())
}
