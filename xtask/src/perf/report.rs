// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Generates offline instruction hotspot reports from a saved run.

mod mappings;
mod render;
mod summary;
mod symbols;

use super::{artifacts, command_output};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    path::Path,
    process::Command,
};

const UNKNOWN: &str = "[unknown]";
type Address = (u64, u64, String);

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
    let (report, warnings) = build(directory, metadata)?;
    fs::write(directory.join("report.log"), warnings.join("\n"))
        .map_err(|error| error.to_string())?;
    fs::write(
        directory.join("report.json"),
        serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    fs::write(directory.join("report.md"), render::markdown(&report)?)
        .map_err(|error| error.to_string())?;
    fs::write(
        directory.join("instructions.md"),
        render::instructions(&report)?,
    )
    .map_err(|error| error.to_string())?;
    println!("Hotspot report: {}", directory.join("report.md").display());
    Ok(())
}

pub(super) fn build(directory: &Path, metadata: &Value) -> Result<(Value, Vec<String>), String> {
    if metadata["mode"] != "profile"
        || !matches!(
            metadata["case"].as_str(),
            Some("calibration" | "system" | "kperf")
        )
    {
        return Err("report requires a saved instruction profile".to_owned());
    }
    check_digest(
        &directory.join("artifacts/manifest.json"),
        &metadata["artifacts"]["sha256"],
    )?;
    check_digest(&directory.join("counts.json"), &metadata["counts_sha256"])?;
    let manifest = read_json(&directory.join("artifacts/manifest.json"))?;
    for name in super::system::image_files(&manifest)? {
        check_digest(
            &directory.join("artifacts").join(&name),
            &manifest["files"][&name],
        )?;
    }
    if manifest["case"] != metadata["case"] || manifest["target"] != metadata["target"] {
        return Err("unsupported performance artifacts".to_owned());
    }
    let counts = read_json(&directory.join("counts.json"))?;
    let system = metadata["case"] != "calibration";
    if system != (counts["scope"] == "system") {
        return Err("instruction scope differs from the saved case".to_owned());
    }
    if system {
        super::system::validate_measurement(&manifest["measurement"])?;
    }
    let addresses = validate(
        &counts,
        metadata["target"]
            .as_str()
            .ok_or("missing profile target")?,
        &manifest,
    )?;
    let mut warnings = Vec::new();
    let locations = symbols::resolve(directory, &manifest, &counts, &addresses, &mut warnings)?;
    let units = if system {
        number(&manifest["measurement"]["units"])?
    } else {
        1
    };
    let windows = counts["windows"]
        .as_array()
        .ok_or("missing profile windows")?
        .iter()
        .map(|window| summarize(&counts, window, &locations, units))
        .collect::<Result<Vec<_>, _>>()?;
    let report = json!({
        "metric": counts["metric"], "scope": counts["scope"],
        "target": metadata["target"], "counts_sha256": metadata["counts_sha256"],
        "contexts": counts["contexts"], "context_register": counts["context_register"], "mappings": counts["mappings"],
        "measurement": manifest["measurement"],
        "saved_images": manifest["images"],
        "artifacts_sha256": metadata["artifacts"]["sha256"],
        "tools": {
            "llvm_symbolizer": command_output(Command::new("llvm-symbolizer").arg("--version"))?,
            "llvm_objdump": command_output(Command::new("llvm-objdump").arg("--version"))?,
        },
        "warnings": warning_summary(&warnings, "report.log"),
        "windows": windows,
    });
    Ok((report, warnings))
}

pub(super) fn read_json(path: &Path) -> Result<Value, String> {
    serde_json::from_slice(&fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?)
        .map_err(|error| format!("{}: {error}", path.display()))
}

pub(super) fn check_digest(path: &Path, expected: &Value) -> Result<(), String> {
    if expected.as_str() != Some(artifacts::digest(path)?.as_str()) {
        return Err(format!("checksum mismatch: {}", path.display()));
    }
    Ok(())
}

pub(super) fn number(value: &Value) -> Result<u64, String> {
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

fn validate(counts: &Value, target: &str, manifest: &Value) -> Result<Vec<Address>, String> {
    if !matches!(
        counts["scope"].as_str(),
        Some("controlled-mappings" | "system")
    ) || counts["valid"] != true
        || counts["plugin_api"] != 7
        || counts["metric"] != "guest_instruction_dispatches"
    {
        return Err("unsupported or invalid instruction profile".to_owned());
    }
    mappings::validate(counts, target, manifest)?;
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
        for edge in ["start_image", "end_image"] {
            let image = window.get(edge).ok_or("missing profile boundary image")?;
            if !image.is_null()
                && (counts["scope"] != "system"
                    || image.as_u64().is_none_or(|index| {
                        manifest["images"]
                            .as_array()
                            .is_none_or(|images| index >= images.len() as u64)
                    }))
            {
                return Err("invalid profile boundary image".to_owned());
            }
        }
        if !names.insert(name) || window["starts"] != 1 || window["ends"] != 1 {
            return Err("invalid profile boundaries".to_owned());
        }
        let rows = window["addresses"]
            .as_array()
            .ok_or("missing profile addresses")?;
        let mut seen = BTreeSet::new();
        let mut total = 0u64;
        for row in rows {
            let context = mappings::context(counts, row, name)?;
            let address = pc(&row["pc"])?;
            let code = row["bytes"]
                .as_str()
                .ok_or("missing profile instruction bytes")?;
            if code.is_empty()
                || code.len() > 32
                || code.len() % 2 != 0
                || !code.bytes().all(|byte| byte.is_ascii_hexdigit())
                || !seen.insert((context, address, code))
                || number(&row["instructions"])? == 0
                || row["disassembly"].as_str().is_none()
                || (address == pc(&window["start"])?
                    && mappings::boundary(counts, context, window, "start"))
                || (address == pc(&window["end"])?
                    && mappings::boundary(counts, context, window, "end"))
            {
                return Err("invalid profile instruction record".to_owned());
            }
            total = total
                .checked_add(number(&row["instructions"])?)
                .ok_or("profile count overflow")?;
            addresses.insert((context, address, code.to_owned()));
        }
        if total != number(&window["instructions"])? {
            return Err("profile does not sum to the interval count".to_owned());
        }
    }
    Ok(addresses.into_iter().collect())
}

fn summarize(
    counts: &Value,
    window: &Value,
    locations: &BTreeMap<Address, Value>,
    work_units: u64,
) -> Result<Value, String> {
    let mut rows = Vec::new();
    for row in window["addresses"]
        .as_array()
        .ok_or("missing profile addresses")?
    {
        let context = mappings::context(
            counts,
            row,
            window["name"].as_str().ok_or("missing window name")?,
        )?;
        let key = (
            context,
            pc(&row["pc"])?,
            row["bytes"]
                .as_str()
                .ok_or("missing instruction bytes")?
                .to_owned(),
        );
        let location = locations.get(&key);
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
            "context": context, "pc": row["pc"], "bytes": row["bytes"],
            "instructions": row["instructions"], "image": location.map(|location| location["image"].as_str().unwrap_or(UNKNOWN)).unwrap_or(UNKNOWN),
            "link_pc": location.map(|location| &location["link_pc"]),
            "segment": location.map(|location| &location["segment"]), "offset": location.map(|location| &location["offset"]),
            "function": function.unwrap_or(UNKNOWN), "source": source.as_deref().unwrap_or(UNKNOWN),
            "function_definition": frame.and_then(|frame| frame["Definition"].as_str()).unwrap_or(""),
            "symbol": frame.and_then(|frame| frame["LinkageName"].as_str()),
            "mnemonic": row["disassembly"].as_str().and_then(|text| text.split_whitespace().next()).unwrap_or(UNKNOWN),
            "inline_frames": frames, "disassembly": location.and_then(|location| location.get("disassembly")).unwrap_or(&row["disassembly"]),
        }));
    }
    rows.sort_by(|left, right| {
        right["instructions"]
            .as_u64()
            .cmp(&left["instructions"].as_u64())
            .then_with(|| left["pc"].as_str().cmp(&right["pc"].as_str()))
            .then_with(|| left["bytes"].as_str().cmp(&right["bytes"].as_str()))
            .then_with(|| left["context"].as_u64().cmp(&right["context"].as_u64()))
    });
    let total = number(&window["instructions"])?;
    Ok(json!({
        "name": window["name"], "instructions": total, "work_units": work_units,
        "instructions_per_unit": total as f64 / work_units as f64,
        "attribution_complete": rows.iter().all(|row| row["image"] != UNKNOWN),
        "images": group(&rows, "image", total, work_units)?, "functions": group(&rows, "function", total, work_units)?,
        "lines": group(&rows, "source", total, work_units)?, "addresses": rows,
        "inline_functions": summary::inline_functions(&rows, total, work_units)?,
        "instruction_classes": summary::instruction_classes(&rows, total, work_units)?,
        "mnemonics": group(&rows, "mnemonic", total, work_units)?,
        "coverage": summary::coverage(&rows, total)?,
    }))
}

fn group(rows: &[Value], field: &str, total: u64, units: u64) -> Result<Vec<Value>, String> {
    let mut groups = BTreeMap::<(&str, &str, &str), u64>::new();
    groups.insert(
        (
            if matches!(field, "image" | "mnemonic") {
                ""
            } else {
                UNKNOWN
            },
            UNKNOWN,
            "",
        ),
        0,
    );
    for row in rows {
        let name = row[field].as_str().ok_or("missing profile group")?;
        let image = if matches!(field, "image" | "mnemonic") {
            ""
        } else {
            row["image"].as_str().ok_or("missing profile image")?
        };
        let definition = if field == "function" {
            row["function_definition"]
                .as_str()
                .ok_or("missing function definition")?
        } else {
            ""
        };
        let value = groups.entry((image, name, definition)).or_default();
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
    groups.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    Ok(groups
        .into_iter()
        .map(|((image, name, definition), count)| {
            let mut group = json!({
                "name": name, "instructions": count, "share_percent": summary::share(count, total),
                "instructions_per_unit": count as f64 / units as f64,
            });
            if !image.is_empty() {
                group["image"] = json!(image);
            }
            if field == "function" {
                group["definition"] = json!(definition);
            }
            group
        })
        .collect())
}

pub(super) fn cell(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('|', "&#124;")
        .replace(['\n', '\r'], " ")
}

pub(super) fn group_label(row: &Value) -> String {
    let mut label = row["name"].as_str().unwrap_or(UNKNOWN).to_owned();
    if let Some(image) = row["image"].as_str().filter(|image| !image.is_empty()) {
        label = format!("{image} / {label}");
    }
    if let Some(definition) = row["definition"].as_str().filter(|name| !name.is_empty()) {
        label.push_str(&format!(" ({definition})"));
    }
    label
}

pub(super) fn warning_summary(warnings: &[String], log: &str) -> Vec<String> {
    let mut messages = BTreeMap::<String, usize>::new();
    for line in warnings
        .iter()
        .flat_map(|warning| warning.lines())
        .filter(|line| !line.is_empty())
    {
        let message =
            if line.contains("address range table") && line.contains("premature terminator") {
                "LLVM reported premature terminators in DWARF address ranges".to_owned()
            } else {
                line.to_owned()
            };
        *messages.entry(message).or_default() += 1;
    }
    messages
        .into_iter()
        .map(|(message, count)| format!("{message} ({count} diagnostics; see {log})"))
        .collect()
}
