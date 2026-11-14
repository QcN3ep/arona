// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Aggregate inline expansions, instruction kinds, and attribution coverage.

use super::{UNKNOWN, number};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn share(count: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        count as f64 / total as f64 * 100.0
    }
}

pub(super) fn inline_functions(
    rows: &[Value],
    total: u64,
    units: u64,
) -> Result<Vec<Value>, String> {
    let mut groups = BTreeMap::<(&str, &str, &str), u64>::new();
    for row in rows {
        let image = row["image"].as_str().ok_or("missing inline image")?;
        let mut seen = BTreeSet::new();
        for frame in row["inline_frames"].as_array().into_iter().flatten() {
            let name = frame["FunctionName"]
                .as_str()
                .ok_or("missing inline name")?;
            if name == UNKNOWN {
                continue;
            }
            let definition = frame["Definition"]
                .as_str()
                .ok_or("missing inline definition")?;
            if !seen.insert((name, definition)) {
                continue;
            }
            let count = groups.entry((image, name, definition)).or_default();
            *count = count
                .checked_add(number(&row["instructions"])?)
                .ok_or("inline count overflow")?;
        }
    }
    let mut result: Vec<_> = groups.into_iter().map(|((image, name, definition), count)| {
        json!({
            "image": image, "name": name, "definition": definition, "instructions": count,
            "share_percent": share(count, total), "instructions_per_unit": count as f64 / units as f64,
        })
    }).collect();
    result.sort_by(|left, right| {
        right["instructions"]
            .as_u64()
            .cmp(&left["instructions"].as_u64())
            .then_with(|| left["name"].as_str().cmp(&right["name"].as_str()))
    });
    Ok(result)
}

pub(super) fn instruction_classes(
    rows: &[Value],
    total: u64,
    units: u64,
) -> Result<Vec<Value>, String> {
    let mut groups = BTreeMap::<&str, u64>::new();
    for row in rows {
        let mnemonic = row["mnemonic"]
            .as_str()
            .ok_or("missing instruction mnemonic")?;
        let class = match mnemonic {
            "sfence.vma" | "invtlb" => "TLB invalidation",
            "tlbfill" => "TLB fill",
            "fence" | "fence.i" | "dbar" | "ibar" => "Memory or instruction barrier",
            "sret" | "mret" | "ertn" => "Exception return",
            name if name.starts_with("amo")
                || name.starts_with("amadd.")
                || name.starts_with("amswap.")
                || name.starts_with("amand.")
                || name.starts_with("amor.")
                || name.starts_with("amxor.")
                || name.starts_with("ammax.")
                || name.starts_with("ammin.")
                || name.starts_with("lr.")
                || name.starts_with("sc.")
                || name.starts_with("ll.") =>
            {
                "Atomic operation"
            }
            _ => continue,
        };
        let count = groups.entry(class).or_default();
        *count = count
            .checked_add(number(&row["instructions"])?)
            .ok_or("instruction class count overflow")?;
    }
    Ok(groups
        .into_iter()
        .map(|(name, count)| {
            json!({
                "name": name, "instructions": count, "share_percent": share(count, total),
                "instructions_per_unit": count as f64 / units as f64,
            })
        })
        .collect())
}

pub(super) fn coverage(rows: &[Value], total: u64) -> Result<Vec<Value>, String> {
    ["image", "function", "source"]
        .into_iter()
        .map(|field| {
            let unknown =
                rows.iter()
                    .filter(|row| row[field] == UNKNOWN)
                    .try_fold(0u64, |sum, row| {
                        sum.checked_add(number(&row["instructions"])?)
                            .ok_or_else(|| "coverage count overflow".to_owned())
                    })?;
            Ok(json!({
                "kind": field, "known": total-unknown, "unknown": unknown,
                "known_share_percent": share(total-unknown, total),
            }))
        })
        .collect()
}
