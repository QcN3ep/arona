// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Render compact hotspot summaries and complete instruction listings.

use super::{UNKNOWN, cell, group_label};
use serde_json::Value;
use std::fmt::Write;

pub(super) fn markdown(report: &Value) -> Result<String, String> {
    let mut text =
        String::from("# Guest instruction hotspots\n\nCounts are guest instruction dispatches.\n");
    if let Some(workload) = report["measurement"]["workload"].as_str() {
        writeln!(text, "\nWorkload: {}.\n", cell(workload)).map_err(|error| error.to_string())?;
    }
    text.push_str("[Instruction details](instructions.md) contain all captured addresses and inline frames. Full data remains in report.json.\n");
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
    if report["scope"] != "system" {
        write_contexts(
            &mut text,
            report["contexts"]
                .as_array()
                .ok_or("missing report contexts")?,
            report["context_register"].as_str(),
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

pub(super) fn instructions(report: &Value) -> Result<String, String> {
    let mut text = String::from(
        "# Instruction details\n\n[Hotspot summary](report.md). The JSON report also retains original linkage names.\n",
    );
    for window in report["windows"]
        .as_array()
        .ok_or("missing report windows")?
    {
        writeln!(
            text,
            "\n## {}\n",
            cell(window["name"].as_str().unwrap_or(UNKNOWN))
        )
        .map_err(|error| error.to_string())?;
        write_groups(
            &mut text,
            window,
            "mnemonics",
            "Instruction mnemonic",
            usize::MAX,
        )
        .map_err(|error| error.to_string())?;
        write_instructions(&mut text, window).map_err(|error| error.to_string())?;
    }
    Ok(text)
}

fn write_window(text: &mut String, window: &Value) -> std::fmt::Result {
    writeln!(
        text,
        "\n## {}\n\nTotal: {} instructions over {} work units, {:.2} instructions per unit.\n",
        cell(window["name"].as_str().unwrap_or(UNKNOWN)),
        window["instructions"],
        window["work_units"],
        window["instructions_per_unit"].as_f64().unwrap_or(0.0)
    )?;
    text.push_str("| Attribution | Known instructions | Unknown instructions | Coverage |\n| --- | ---: | ---: | ---: |\n");
    for row in window["coverage"].as_array().into_iter().flatten() {
        writeln!(
            text,
            "| {} | {} | {} | {:.2}% |",
            cell(row["kind"].as_str().unwrap_or(UNKNOWN)),
            row["known"],
            row["unknown"],
            row["known_share_percent"].as_f64().unwrap_or(0.0)
        )?;
    }
    text.push_str("\nThe following tables show up to 20 entries per group. Percentages refer to instruction counts.\n");
    write_groups(text, window, "instruction_classes", "Instruction kind", 20)?;
    text.push_str("\nDirect function and source groups assign each instruction to its innermost inline frame. Each complete grouping sums to the total.\n");
    for (field, title) in [
        ("images", "Image"),
        ("functions", "Direct function"),
        ("lines", "Source line"),
    ] {
        write_groups(text, window, field, title, 20)?;
    }
    text.push_str("\nInline expansion totals include an instruction in each distinct function in its inline chain. Rows overlap and do not sum to the total. These relationships describe compiler inlining. Out-of-line callees are counted separately.\n");
    write_groups(
        text,
        window,
        "inline_functions",
        "Function including inline expansions",
        20,
    )
}

fn write_groups(
    text: &mut String,
    window: &Value,
    field: &str,
    title: &str,
    limit: usize,
) -> std::fmt::Result {
    writeln!(
        text,
        "\n| {title} | Instructions | Per unit | Share |\n| --- | ---: | ---: | ---: |"
    )?;
    for row in window[field]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row["instructions"] != 0)
        .take(limit)
    {
        writeln!(
            text,
            "| {} | {} | {:.2} | {:.2}% |",
            cell(&group_label(row)),
            row["instructions"],
            row["instructions_per_unit"].as_f64().unwrap_or(0.0),
            row["share_percent"].as_f64().unwrap_or(0.0)
        )?;
    }
    Ok(())
}

fn write_contexts(
    text: &mut String,
    contexts: &[Value],
    root_register: Option<&str>,
) -> std::fmt::Result {
    writeln!(
        text,
        "\nRoot register: {}.\n",
        root_register.unwrap_or("unavailable")
    )?;
    if root_register.is_none() {
        text.push_str("Page-table roots are not independently observed. Attribution relies on the calibration driver's controlled descriptors.\n\n");
    }
    text.push_str("| Context | Load instance | Declared root | Observed root | Runtime base | ELF base |\n| --- | --- | --- | --- | --- | --- |\n");
    let mut unknown = 0;
    for context in contexts.iter().skip(1) {
        if !context["reason"].is_null() {
            unknown += 1;
            continue;
        }
        writeln!(
            text,
            "| {} | {} | {} | {} | {} | {} |",
            context["id"],
            context["instance"],
            context["root"].as_str().unwrap_or(UNKNOWN),
            context["observed_root"].as_str().unwrap_or(UNKNOWN),
            context["runtime"].as_str().unwrap_or(UNKNOWN),
            context["link"].as_str().unwrap_or(UNKNOWN)
        )?;
    }
    writeln!(
        text,
        "\nUnresolved execution observations: {unknown}. Details remain in report.json."
    )
}

fn write_instructions(text: &mut String, window: &Value) -> std::fmt::Result {
    text.push_str("\n| Context | PC | ELF PC | Segment offset | Bytes | Instructions | Disassembly | Inline frames |\n| --- | --- | --- | --- | --- | ---: | --- | --- |\n");
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
            "| {} | {} | {} | {}:{} | {} | {} | {} | {} |",
            row["context"],
            cell(row["pc"].as_str().unwrap_or(UNKNOWN)),
            cell(row["link_pc"].as_str().unwrap_or(UNKNOWN)),
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
