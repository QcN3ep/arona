// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Interprets observed calibration and native executable mappings.

use super::{number, pc};
use serde_json::Value;
use std::collections::BTreeSet;

pub(super) fn validate(counts: &Value, target: &str, manifest: &Value) -> Result<(), String> {
    if counts["scope"] == "system" {
        return validate_system(counts, target, manifest);
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

fn validate_system(counts: &Value, target: &str, manifest: &Value) -> Result<(), String> {
    let register = if target == "riscv64gc-unknown-none-elf" {
        "satp"
    } else {
        "csrwr-pgdl"
    };
    let contexts = counts["contexts"]
        .as_array()
        .ok_or("missing system contexts")?;
    if contexts.get(..2)
        != Some(
            &[
                serde_json::json!({"id":0,"kind":"shared-kernel"}),
                serde_json::json!({"id":1,"kind":"unknown"}),
            ][..],
        )
        || counts["context_register"] != register
    {
        return Err("invalid system execution contexts".to_owned());
    }
    let images = manifest["images"]
        .as_array()
        .ok_or("missing saved images")?;
    if contexts.len() != images.len() + 2 {
        return Err("unexpected native image contexts".to_owned());
    }
    for (index, context) in contexts.iter().enumerate().skip(2) {
        if *context != serde_json::json!({"id":index,"kind":"native-image","image":index-2}) {
            return Err("invalid native image context".to_owned());
        }
    }
    let mut epochs = BTreeSet::new();
    for context in counts["mappings"]
        .as_array()
        .ok_or("missing observed mappings")?
    {
        let root = pc(&context["root"])?;
        let epoch = number(&context["epoch"])?;
        let valid_root = if register == "satp" {
            root >> 60 == 8
        } else {
            root != 0 && root & 4095 == 0
        };
        if !valid_root || epoch == 0 || !epochs.insert(epoch) {
            return Err("invalid native mapping context".to_owned());
        }
        if context["image"].is_null() {
            if context["reason"].as_str().is_none_or(str::is_empty) {
                return Err("missing unresolved mapping reason".to_owned());
            }
        } else if number(&context["image"])? >= images.len() as u64 || !context["reason"].is_null()
        {
            return Err("invalid native mapping image".to_owned());
        }
    }
    Ok(())
}

pub(super) fn context(counts: &Value, row: &Value, window: &str) -> Result<u64, String> {
    let id = number(&row["context"])?;
    if counts["scope"] == "system" {
        let shared = pc(&row["pc"])? >= 0xffff_0000_0000_0000;
        if window != "workload"
            || (shared && id != 0)
            || counts["contexts"]
                .as_array()
                .and_then(|contexts| contexts.get(id as usize))
                .is_none()
        {
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
    manifest: &Value,
) -> Option<(u64, String, String)> {
    if context == 0 {
        return Some((address, "guest.elf".to_owned(), String::new()));
    }
    if counts["scope"] == "system" {
        let context = counts["contexts"]
            .as_array()?
            .get(usize::try_from(context).ok()?)?;
        let image = manifest["images"]
            .as_array()?
            .get(usize::try_from(context["image"].as_u64()?).ok()?)?;
        return Some((
            address,
            image["name"].as_str()?.to_owned(),
            image["directory"].as_str()?.to_owned(),
        ));
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
            String::new(),
        ));
    }
    // The calibration roots retain one shared higher-half kernel mapping.
    (address >= 0xffff_0000_0000_0000).then(|| (address, "guest.elf".to_owned(), String::new()))
}

pub(super) fn boundary(counts: &Value, context: u64, window: &Value, edge: &str) -> bool {
    let image = &window[format!("{edge}_image")];
    if image.is_null() {
        return context == 0;
    }
    counts["contexts"]
        .as_array()
        .and_then(|contexts| contexts.get(context as usize))
        .is_some_and(|context| context["image"] == *image && context["reason"].is_null())
}
