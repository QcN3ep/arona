// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Interprets observed contexts from the controlled mapping calibration.

use super::{number, pc};
use serde_json::Value;
use std::collections::BTreeSet;

pub(super) fn validate(counts: &Value, target: &str) -> Result<(), String> {
    if counts["scope"] == "system" {
        if counts["contexts"]
            != serde_json::json!([
                {"id":0,"kind":"shared-kernel"}, {"id":1,"kind":"unknown"}
            ])
            || !counts["context_register"].is_null()
        {
            return Err("invalid system execution contexts".to_owned());
        }
        return Ok(());
    }
    let require_root = target == "riscv64gc-unknown-none-elf";
    if !counts["context_register"].is_null()
        && (!require_root || counts["context_register"] != "satp")
    {
        return Err("unsupported context register".to_owned());
    }
    let contexts = counts["contexts"]
        .as_array()
        .ok_or("missing execution contexts")?;
    if contexts
        .first()
        .is_none_or(|context| context["id"] != 0 || context["kind"] != "static")
    {
        return Err("missing static calibration context".to_owned());
    }
    let mut instances = BTreeSet::new();
    for (id, context) in contexts.iter().enumerate().skip(1) {
        if context["id"] != id || context["kind"] != "descriptor" {
            return Err("invalid execution context identifier".to_owned());
        }
        if !context["reason"].is_null() {
            context["reason"]
                .as_str()
                .ok_or("invalid context failure reason")?;
            continue;
        }
        let instance = number(&context["instance"])?;
        if !instances.insert(instance) || pc(&context["root"])? == 0 {
            return Err("reused or invalid load instance".to_owned());
        }
        if require_root
            && (counts["context_register"] != "satp" || context["root"] != context["observed_root"])
        {
            return Err("unverified page-table root".to_owned());
        }
        if instance == 0 {
            if pc(&context["runtime"])? != 0 || pc(&context["link"])? != 0 || context["size"] != 0 {
                return Err("invalid baseline mapping descriptor".to_owned());
            }
        } else if pc(&context["runtime"])? != 0x10000
            || number(&context["size"])? != 12
            || pc(&context["link"])? == 0
        {
            return Err("unsupported controlled image mapping".to_owned());
        }
    }
    Ok(())
}

pub(super) fn context(counts: &Value, row: &Value, window: &str) -> Result<u64, String> {
    let id = number(&row["context"])?;
    if counts["scope"] == "system" {
        let shared = pc(&row["pc"])? >= 0xffff_0000_0000_0000;
        if window != "workload" || id != u64::from(!shared) {
            return Err("invalid system instruction context".to_owned());
        }
        return Ok(id);
    }
    let index = usize::try_from(id).map_err(|error| error.to_string())?;
    if counts["contexts"]
        .as_array()
        .and_then(|contexts| contexts.get(index))
        .is_none()
        || (window == "mappings") == (id == 0)
    {
        return Err("instruction has an invalid execution context".to_owned());
    }
    Ok(id)
}

pub(super) fn relocate(
    counts: &Value,
    context: u64,
    address: u64,
    instruction_size: usize,
) -> Option<(u64, String)> {
    if context == 0 {
        return Some((address, "guest.elf".to_owned()));
    }
    if counts["scope"] == "system" {
        return None;
    }
    let descriptor = counts["contexts"]
        .as_array()?
        .get(usize::try_from(context).ok()?)?;
    if !descriptor["reason"].is_null() {
        return None;
    }
    let runtime = pc(&descriptor["runtime"]).ok()?;
    let size = number(&descriptor["size"]).ok()?;
    let instruction_size = u64::try_from(instruction_size).ok()?;
    if let Some(offset) = address.checked_sub(runtime).filter(|offset| {
        offset
            .checked_add(instruction_size)
            .is_some_and(|end| end <= size)
    }) {
        return Some((
            pc(&descriptor["link"]).ok()?.checked_add(offset)?,
            format!("guest.elf@{}", descriptor["instance"]),
        ));
    }
    // The calibration roots retain one shared higher-half kernel mapping.
    (address >= 0xffff_0000_0000_0000).then(|| (address, "guest.elf".to_owned()))
}
