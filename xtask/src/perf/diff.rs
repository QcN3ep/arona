// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Compares saved instruction measurements without running or rebuilding the guest.

use super::{Target, artifacts, read_windows, report, system, validate_counts};
use report::{cell, check_digest, number, read_json};
use serde_json::{Value, json};
use std::{collections::BTreeMap, ffi::OsString, fmt::Write, fs, path::Path};

const UNKNOWN: &str = "[unknown]";
type Groups = BTreeMap<(String, String), u64>;

pub(super) fn run(mut arguments: impl Iterator<Item = OsString>) -> Result<(), String> {
    let before = arguments.next().ok_or_else(super::super::usage)?;
    let after = arguments.next().ok_or_else(super::super::usage)?;
    if arguments.next().is_some() {
        return Err(super::super::usage());
    }
    let before = Path::new(&before);
    let after = Path::new(&after);
    let left = SavedRun::load(before)?;
    let right = SavedRun::load(after)?;
    compatible(&left, &right)?;
    let windows = left
        .windows
        .iter()
        .zip(&right.windows)
        .map(|(left, right)| compare_window(left, right))
        .collect::<Result<Vec<_>, _>>()?;
    let comparison = json!({
        "metric": "guest_instruction_dispatches", "target": left.metadata["target"],
        "case": left.metadata["case"], "mode": left.metadata["mode"],
        "before": identity(before, &left), "after": identity(after, &right),
        "windows": windows,
    });
    fs::write(
        after.join("diff.json"),
        serde_json::to_vec_pretty(&comparison).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    fs::write(after.join("diff.md"), markdown(&comparison)?).map_err(|error| error.to_string())?;
    println!(
        "Instruction comparison: {}",
        after.join("diff.md").display()
    );
    Ok(())
}

struct SavedRun {
    metadata: Value,
    manifest: Value,
    windows: Vec<Value>,
    warnings: Vec<String>,
}

impl SavedRun {
    fn load(directory: &Path) -> Result<Self, String> {
        let metadata = read_json(&directory.join("run.json"))?;
        if metadata["status"] != "complete"
            || metadata["plugin_api"] != 7
            || !matches!(metadata["mode"].as_str(), Some("count" | "profile"))
            || !matches!(metadata["case"].as_str(), Some("calibration" | "system"))
        {
            return Err(format!(
                "incomplete or invalid performance run: {}",
                directory.display()
            ));
        }
        check_digest(&directory.join("counts.json"), &metadata["counts_sha256"])?;
        check_digest(
            &directory.join("artifacts/manifest.json"),
            &metadata["artifacts"]["sha256"],
        )?;
        for (file, key) in [
            ("count.c", "source_sha256"),
            ("context.h", "context_header_sha256"),
            ("count.so", "plugin_sha256"),
        ] {
            check_digest(&directory.join(file), &metadata["collector"][key])?;
        }
        let manifest = read_json(&directory.join("artifacts/manifest.json"))?;
        validate_artifacts(directory, &metadata, &manifest)?;
        let (expected, _) = read_windows(&directory.join("artifacts/symbols.elf"), &manifest)?;
        validate_counts(directory, &expected)?;
        let (windows, warnings) = if metadata["mode"] == "profile" {
            let (report, warnings) = report::build(directory, &metadata)?;
            (
                report["windows"]
                    .as_array()
                    .ok_or("missing report windows")?
                    .clone(),
                warnings,
            )
        } else {
            let counts = read_json(&directory.join("counts.json"))?;
            let mut windows = counts["windows"]
                .as_array()
                .ok_or("missing count windows")?
                .clone();
            let units = if metadata["case"] == "system" {
                number(&manifest["measurement"]["units"])?
            } else {
                1
            };
            for window in &mut windows {
                window["work_units"] = json!(units);
            }
            (windows, Vec::new())
        };
        Ok(Self {
            metadata,
            manifest,
            windows,
            warnings,
        })
    }
}

fn validate_artifacts(directory: &Path, metadata: &Value, manifest: &Value) -> Result<(), String> {
    for key in ["target", "case", "release"] {
        if manifest[key].is_null() || manifest[key] != metadata[key] {
            return Err(format!("saved run and artifacts disagree on {key}"));
        }
    }
    let target = match manifest["target"].as_str() {
        Some("riscv64gc-unknown-none-elf") => Target::Riscv64,
        Some("loongarch64-unknown-none") => Target::LoongArch64,
        _ => return Err("unsupported performance target".to_owned()),
    };
    let mut files = artifacts::filenames(target, manifest["case"].as_str().ok_or("missing case")?)
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    files.extend(system::image_files(manifest)?);
    if manifest["files"].as_object().map(|files| files.len()) != Some(files.len()) {
        return Err("unexpected performance artifact files".to_owned());
    }
    for name in files {
        check_digest(
            &directory.join("artifacts").join(&name),
            &manifest["files"][&name],
        )?;
    }
    if metadata["case"] == "system" {
        system::validate_measurement(&manifest["measurement"])?;
        if metadata["measurement"] != manifest["measurement"] {
            return Err("saved measurement settings disagree".to_owned());
        }
    }
    Ok(())
}

fn compatible(left: &SavedRun, right: &SavedRun) -> Result<(), String> {
    for key in ["target", "case", "mode", "plugin_api", "qemu_version"] {
        if left.metadata[key].is_null() || left.metadata[key] != right.metadata[key] {
            return Err(format!("performance runs differ in {key}"));
        }
    }
    for key in ["source_sha256", "context_header_sha256", "plugin_sha256"] {
        if left.metadata["collector"][key] != right.metadata["collector"][key] {
            return Err(format!("performance collectors differ in {key}"));
        }
    }
    if left.manifest["machine_arguments"].as_array().is_none()
        || left.manifest["machine_arguments"] != right.manifest["machine_arguments"]
        || left.manifest["files"]["firmware.bin"] != right.manifest["files"]["firmware.bin"]
    {
        return Err("performance runs use different QEMU machine settings or firmware".to_owned());
    }
    if left.manifest["measurement"] != right.manifest["measurement"] {
        return Err("performance runs use different workloads, units, or boundaries".to_owned());
    }
    if left.windows.len() != right.windows.len()
        || left
            .windows
            .iter()
            .zip(&right.windows)
            .any(|(left, right)| left["name"] != right["name"])
    {
        return Err("performance runs use different measurement windows".to_owned());
    }
    Ok(())
}

fn identity(directory: &Path, run: &SavedRun) -> Value {
    json!({
        "directory": directory, "counts_sha256": run.metadata["counts_sha256"],
        "artifacts_sha256": run.metadata["artifacts"]["sha256"],
        "release": run.metadata["release"], "measurement": run.manifest["measurement"],
        "build": run.manifest["build"], "warnings": run.warnings,
    })
}

fn change(before: u64, after: u64, units: u64) -> Value {
    let delta = i128::from(after) - i128::from(before);
    json!({
        "before": before, "after": after, "delta": delta.to_string(),
        "change_percent": if before == 0 { None } else { Some(delta as f64 / before as f64 * 100.0) },
        "before_per_unit": before as f64 / units as f64,
        "after_per_unit": after as f64 / units as f64,
        "delta_per_unit": delta as f64 / units as f64,
    })
}

fn compare_window(left: &Value, right: &Value) -> Result<Value, String> {
    let units = number(&left["work_units"])?;
    if units == 0 || right["work_units"] != units {
        return Err("performance windows use different work units".to_owned());
    }
    let before = number(&left["instructions"])?;
    let after = number(&right["instructions"])?;
    let mut window = change(before, after, units);
    window["name"] = left["name"].clone();
    window["work_units"] = json!(units);
    if left["addresses"].is_array() {
        for (field, label) in [
            ("image", "images"),
            ("function", "functions"),
            ("source", "lines"),
        ] {
            let left = groups(left, field)?;
            let right = groups(right, field)?;
            let mut unknown = change(unknown_count(&left)?, unknown_count(&right)?, units);
            unknown["before_share_percent"] = json!(share(number(&unknown["before"])?, before));
            unknown["after_share_percent"] = json!(share(number(&unknown["after"])?, after));
            window["unknown"][label] = unknown;
            window[label] = json!(compare_groups(&left, &right, units));
        }
    }
    Ok(window)
}

fn share(count: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        count as f64 / total as f64 * 100.0
    }
}

fn groups(window: &Value, field: &str) -> Result<Groups, String> {
    let mut groups = Groups::new();
    groups.insert((UNKNOWN.to_owned(), UNKNOWN.to_owned()), 0);
    for row in window["addresses"]
        .as_array()
        .ok_or("missing hotspot addresses")?
    {
        let image = row["image"].as_str().ok_or("missing hotspot image")?;
        let name = row[field].as_str().ok_or("missing hotspot name")?;
        let count = groups
            .entry((image.to_owned(), name.to_owned()))
            .or_default();
        *count = count
            .checked_add(number(&row["instructions"])?)
            .ok_or("hotspot count overflow")?;
    }
    Ok(groups)
}

fn unknown_count(groups: &Groups) -> Result<u64, String> {
    groups
        .iter()
        .filter(|((_, name), _)| name == UNKNOWN)
        .try_fold(0u64, |sum, (_, count)| sum.checked_add(*count))
        .ok_or_else(|| "unknown count overflow".to_owned())
}

fn compare_groups(left: &Groups, right: &Groups, units: u64) -> Vec<Value> {
    let mut keys = left.keys().chain(right.keys()).collect::<Vec<_>>();
    keys.sort();
    keys.dedup();
    keys.sort_by_key(|key| {
        let delta =
            i128::from(*right.get(*key).unwrap_or(&0)) - i128::from(*left.get(*key).unwrap_or(&0));
        std::cmp::Reverse(delta.abs())
    });
    keys.into_iter()
        .map(|key| {
            let before = left.get(key);
            let after = right.get(key);
            let mut row = change(
                before.copied().unwrap_or(0),
                after.copied().unwrap_or(0),
                units,
            );
            row["image"] = json!(key.0);
            row["name"] = json!(key.1);
            row["status"] = json!(match (before, after) {
                (None, _) => "added",
                (_, None) => "removed",
                _ => "matched",
            });
            row
        })
        .collect()
}

fn markdown(comparison: &Value) -> Result<String, String> {
    let mut text = format!(
        "# Guest instruction comparison\n\nBefore: {}\n\nAfter: {}\n\nChanges are after minus before. Counts are guest instruction dispatches.\n\nGroups match exact image names and function names or source locations. Unmatched entries are added or removed. Absolute PCs are not matched across builds.\n",
        cell(
            comparison["before"]["directory"]
                .as_str()
                .ok_or("missing before directory")?
        ),
        cell(
            comparison["after"]["directory"]
                .as_str()
                .ok_or("missing after directory")?
        ),
    );
    for side in ["before", "after"] {
        for warning in comparison[side]["warnings"]
            .as_array()
            .ok_or("missing comparison warnings")?
        {
            writeln!(
                text,
                "\n{side} warning: {}",
                cell(warning.as_str().ok_or("invalid warning")?)
            )
            .map_err(|error| error.to_string())?;
        }
    }
    for window in comparison["windows"]
        .as_array()
        .ok_or("missing comparison windows")?
    {
        write_window(&mut text, window).map_err(|error| error.to_string())?;
    }
    Ok(text)
}

fn write_window(text: &mut String, window: &Value) -> std::fmt::Result {
    writeln!(
        text,
        "\n## {}\n\nWork units: {}.\n",
        cell(window["name"].as_str().unwrap_or(UNKNOWN)),
        window["work_units"]
    )?;
    text.push_str("| Metric | Before | After | Change |\n| --- | ---: | ---: | ---: |\n");
    writeln!(
        text,
        "| Instructions | {} | {} | {} |",
        window["before"],
        window["after"],
        window["delta"].as_str().unwrap_or(UNKNOWN)
    )?;
    writeln!(
        text,
        "| Instructions per unit | {} | {} | {} |",
        window["before_per_unit"], window["after_per_unit"], window["delta_per_unit"]
    )?;
    if window["images"].is_array() {
        text.push_str("\nUnknown counts can move between groups as attribution changes. Check their coverage alongside the total.\n\n| Unknown attribution | Before | After | Before % | After % |\n| --- | ---: | ---: | ---: | ---: |\n");
        for field in ["images", "functions", "lines"] {
            let row = &window["unknown"][field];
            writeln!(
                text,
                "| {field} | {} | {} | {} | {} |",
                row["before"],
                row["after"],
                row["before_share_percent"],
                row["after_share_percent"]
            )?;
        }
        text.push_str("\nThe following tables show up to 20 entries with the largest absolute count changes. The JSON file contains every entry.\n");
        for (field, title) in [
            ("images", "Image"),
            ("functions", "Function or symbol"),
            ("lines", "Source line"),
        ] {
            writeln!(
                text,
                "\n| {title} | Status | Before | After | Change |\n| --- | --- | ---: | ---: | ---: |"
            )?;
            for row in window[field]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|row| row["delta"] != "0" || row["status"] != "matched")
                .take(20)
            {
                writeln!(
                    text,
                    "| {} | {} | {} | {} | {} |",
                    cell(&if field == "images" {
                        row["name"].as_str().unwrap_or(UNKNOWN).to_owned()
                    } else {
                        format!(
                            "{} / {}",
                            row["image"].as_str().unwrap_or(UNKNOWN),
                            row["name"].as_str().unwrap_or(UNKNOWN)
                        )
                    }),
                    row["status"].as_str().unwrap_or(UNKNOWN),
                    row["before"],
                    row["after"],
                    row["delta"].as_str().unwrap_or(UNKNOWN)
                )?;
            }
        }
    }
    Ok(())
}
