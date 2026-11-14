// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Generates offline instruction hotspot reports from a saved calibration run.

mod symbols;

use super::{artifacts, command_output};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fmt::Write,
    fs,
    path::Path,
    process::Command,
};

const UNKNOWN: &str = "[unknown]";

pub(super) fn run(mut arguments: impl Iterator<Item = OsString>) -> Result<(), String> {
    let directory = arguments.next().ok_or_else(super::super::usage)?;
    if arguments.next().is_some() {
        return Err(super::super::usage());
    }
    let directory = Path::new(&directory);
    let metadata = read_json(&directory.join("run.json"))?;
    if metadata["status"] != "complete" {
        return Err("cannot report an incomplete performance run".to_owned());
    }
    generate(directory, &metadata)
}

pub(super) fn generate(directory: &Path, metadata: &Value) -> Result<(), String> {
    if metadata["schema"] != 2 || metadata["mode"] != "profile" || metadata["case"] != "calibration"
    {
        return Err("report requires a saved calibration profile".to_owned());
    }
    check_digest(
        &directory.join("artifacts/manifest.json"),
        &metadata["artifacts"]["sha256"],
    )?;
    check_digest(&directory.join("counts.json"), &metadata["counts_sha256"])?;
    let manifest = read_json(&directory.join("artifacts/manifest.json"))?;
    if manifest["schema"] != 1
        || manifest["case"] != "calibration"
        || manifest["target"] != metadata["target"]
    {
        return Err("unsupported performance artifacts".to_owned());
    }
    let counts = read_json(&directory.join("counts.json"))?;
    let addresses = validate(&counts)?;
    let mut warnings = Vec::new();
    let locations = symbols::resolve(directory, &manifest, &addresses, &mut warnings)?;
    let windows = counts["windows"]
        .as_array()
        .ok_or("missing profile windows")?
        .iter()
        .map(|window| summarize(window, &locations))
        .collect::<Result<Vec<_>, _>>()?;
    let report = json!({
        "schema": 1, "metric": counts["metric"], "scope": counts["scope"],
        "target": metadata["target"], "counts_sha256": metadata["counts_sha256"],
        "artifacts_sha256": metadata["artifacts"]["sha256"],
        "tools": {
            "llvm_symbolizer": command_output(Command::new("llvm-symbolizer").arg("--version"))?,
            "llvm_objdump": command_output(Command::new("llvm-objdump").arg("--version"))?,
        },
        "warnings": warnings.iter().map(|message| {
            let first = message.lines().next().unwrap_or("");
            if message.lines().count() > 1 { format!("{first} (see report.log)") } else { first.to_owned() }
        }).collect::<Vec<_>>(),
        "windows": windows,
    });
    fs::write(directory.join("report.log"), warnings.join("\n"))
        .map_err(|error| error.to_string())?;
    fs::write(
        directory.join("report.json"),
        serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    fs::write(directory.join("report.md"), markdown(&report)?)
        .map_err(|error| error.to_string())?;
    println!("Hotspot report: {}", directory.join("report.md").display());
    Ok(())
}

fn read_json(path: &Path) -> Result<Value, String> {
    serde_json::from_slice(&fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?)
        .map_err(|error| format!("{}: {error}", path.display()))
}

fn check_digest(path: &Path, expected: &Value) -> Result<(), String> {
    if expected.as_str() != Some(artifacts::digest(path)?.as_str()) {
        return Err(format!("checksum mismatch: {}", path.display()));
    }
    Ok(())
}

fn number(value: &Value) -> Result<u64, String> {
    value
        .as_u64()
        .ok_or_else(|| "invalid profile instruction count".to_owned())
}

fn pc(value: &Value) -> Result<u64, String> {
    value
        .as_str()
        .and_then(|value| value.strip_prefix("0x"))
        .and_then(|value| u64::from_str_radix(value, 16).ok())
        .ok_or_else(|| "invalid profile address".to_owned())
}

fn validate(counts: &Value) -> Result<Vec<(u64, String)>, String> {
    if counts["schema"] != 2
        || counts["valid"] != true
        || counts["plugin_api"] != 7
        || counts["metric"] != "guest_instruction_dispatches"
        || counts["scope"] != "static-calibration"
    {
        return Err("unsupported or invalid instruction profile".to_owned());
    }
    let windows = counts["windows"]
        .as_array()
        .ok_or("missing profile windows")?;
    if windows.is_empty() {
        return Err("empty instruction profile".to_owned());
    }
    let mut addresses = BTreeSet::new();
    let mut names = BTreeSet::new();
    for window in windows {
        let name = window["name"]
            .as_str()
            .ok_or("missing profile window name")?;
        if !names.insert(name) || window["starts"] != 1 || window["ends"] != 1 {
            return Err("invalid profile boundaries".to_owned());
        }
        let rows = window["addresses"]
            .as_array()
            .ok_or("missing profile addresses")?;
        let mut seen = BTreeSet::new();
        let mut total = 0u64;
        for row in rows {
            let address = pc(&row["pc"])?;
            let code = row["bytes"]
                .as_str()
                .ok_or("missing profile instruction bytes")?;
            if code.is_empty()
                || code.len() > 32
                || code.len() % 2 != 0
                || !code.bytes().all(|byte| byte.is_ascii_hexdigit())
                || !seen.insert((address, code))
                || number(&row["instructions"])? == 0
                || row["disassembly"].as_str().is_none()
                || address == pc(&window["start"])?
                || address == pc(&window["end"])?
            {
                return Err("invalid profile instruction record".to_owned());
            }
            total = total
                .checked_add(number(&row["instructions"])?)
                .ok_or("profile count overflow")?;
            addresses.insert((address, code.to_owned()));
        }
        if total != number(&window["instructions"])? {
            return Err("profile does not sum to the interval count".to_owned());
        }
    }
    Ok(addresses.into_iter().collect())
}

fn summarize(window: &Value, locations: &BTreeMap<u64, Value>) -> Result<Value, String> {
    let mut rows = Vec::new();
    for row in window["addresses"]
        .as_array()
        .ok_or("missing profile addresses")?
    {
        let location = locations
            .get(&pc(&row["pc"])?)
            .filter(|location| location["bytes"] == row["bytes"]);
        let frames = location.and_then(|location| location["frames"].as_array());
        let frame = frames.and_then(|frames| frames.first());
        let function = frame
            .and_then(|frame| frame["FunctionName"].as_str())
            .filter(|name| !name.is_empty() && *name != "??");
        let file = frame
            .and_then(|frame| frame["FileName"].as_str())
            .filter(|file| !file.is_empty() && *file != "??");
        let line = frame
            .and_then(|frame| frame["Line"].as_u64())
            .filter(|line| *line > 0);
        let source = file.zip(line).map(|(file, line)| format!("{file}:{line}"));
        rows.push(json!({
            "context": "static-calibration", "pc": row["pc"], "bytes": row["bytes"],
            "instructions": row["instructions"], "image": location.map(|_| "guest.elf").unwrap_or(UNKNOWN),
            "segment": location.map(|location| &location["segment"]), "offset": location.map(|location| &location["offset"]),
            "function": function.unwrap_or(UNKNOWN), "source": source.as_deref().unwrap_or(UNKNOWN),
            "inline_frames": frames, "disassembly": location.and_then(|location| location.get("disassembly")).unwrap_or(&row["disassembly"]),
        }));
    }
    rows.sort_by(|left, right| {
        right["instructions"]
            .as_u64()
            .cmp(&left["instructions"].as_u64())
            .then_with(|| left["pc"].as_str().cmp(&right["pc"].as_str()))
            .then_with(|| left["bytes"].as_str().cmp(&right["bytes"].as_str()))
    });
    let total = number(&window["instructions"])?;
    Ok(json!({
        "name": window["name"], "instructions": total, "work_units": 1,
        "images": group(&rows, "image", total)?, "functions": group(&rows, "function", total)?,
        "lines": group(&rows, "source", total)?, "addresses": rows,
    }))
}

fn group(rows: &[Value], field: &str, total: u64) -> Result<Vec<Value>, String> {
    let mut groups = BTreeMap::<&str, u64>::new();
    groups.insert(UNKNOWN, 0);
    for row in rows {
        let name = row[field].as_str().ok_or("missing profile group")?;
        let value = groups.entry(name).or_default();
        *value = value
            .checked_add(number(&row["instructions"])?)
            .ok_or("profile count overflow")?;
    }
    if groups
        .values()
        .try_fold(0u64, |sum, count| sum.checked_add(*count))
        != Some(total)
    {
        return Err("profile groups do not sum to the interval count".to_owned());
    }
    let mut groups = groups.into_iter().collect::<Vec<_>>();
    groups.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(right.0)));
    Ok(groups
        .into_iter()
        .map(|(name, count)| json!({"name": name, "instructions": count}))
        .collect())
}

fn cell(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('|', "&#124;")
        .replace(['\n', '\r'], " ")
}

fn markdown(report: &Value) -> Result<String, String> {
    let mut text = String::from(
        "# Guest instruction hotspots\n\nCounts are guest instruction dispatches. Each calibration interval is one work unit.\n\nThis report covers the calibration image's fixed boot mapping. It does not resolve multiple address spaces.\n\nFunction and line groups count each instruction once at its innermost frame. Enclosing inline frames are context only.\n",
    );
    for warning in report["warnings"]
        .as_array()
        .ok_or("missing report warnings")?
    {
        writeln!(
            text,
            "\nWarning: {}",
            cell(warning.as_str().ok_or("invalid warning")?)
        )
        .map_err(|error| error.to_string())?;
    }
    for window in report["windows"]
        .as_array()
        .ok_or("missing report windows")?
    {
        write_window(&mut text, window).map_err(|error| error.to_string())?;
    }
    Ok(text)
}

fn write_window(text: &mut String, window: &Value) -> std::fmt::Result {
    writeln!(
        text,
        "\n## {}\n\nTotal: {} instructions, {} instructions per interval.\n",
        window["name"].as_str().unwrap_or(UNKNOWN),
        window["instructions"],
        window["instructions"]
    )?;
    for (field, title) in [
        ("images", "Image"),
        ("functions", "Function or symbol"),
        ("lines", "Source line"),
    ] {
        writeln!(text, "| {title} | Instructions |\n| --- | ---: |")?;
        for row in window[field].as_array().into_iter().flatten() {
            writeln!(
                text,
                "| {} | {} |",
                cell(row["name"].as_str().unwrap_or(UNKNOWN)),
                row["instructions"]
            )?;
        }
        text.push('\n');
    }
    text.push_str("| PC | Segment offset | Bytes | Instructions | Disassembly | Inline frames |\n| --- | --- | --- | ---: | --- | --- |\n");
    for row in window["addresses"].as_array().into_iter().flatten() {
        let frames = row["inline_frames"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|frame| {
                format!(
                    "{} ({}:{})",
                    frame["FunctionName"].as_str().unwrap_or(UNKNOWN),
                    frame["FileName"].as_str().unwrap_or(UNKNOWN),
                    frame["Line"]
                )
            })
            .collect::<Vec<_>>()
            .join(" → ");
        writeln!(
            text,
            "| {} | {}:{} | {} | {} | {} | {} |",
            cell(row["pc"].as_str().unwrap_or(UNKNOWN)),
            row["segment"],
            cell(row["offset"].as_str().unwrap_or(UNKNOWN)),
            cell(row["bytes"].as_str().unwrap_or(UNKNOWN)),
            row["instructions"],
            cell(row["disassembly"].as_str().unwrap_or(UNKNOWN)),
            cell(&frames)
        )?;
    }
    Ok(())
}
