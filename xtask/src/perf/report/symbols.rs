// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Resolves saved static-image addresses through LLVM tools without consulting other builds.

use super::super::{artifacts, command_output};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path, process::Command};

pub(super) fn resolve(
    directory: &Path,
    manifest: &Value,
    addresses: &[(u64, String)],
    warnings: &mut Vec<String>,
) -> Result<BTreeMap<u64, Value>, String> {
    let saved = directory.join("artifacts");
    let mut locations = BTreeMap::new();
    if !available(&saved, "guest.elf", manifest, warnings)? {
        return Ok(locations);
    }
    let guest = saved.join("guest.elf");
    let image = artifacts::inspect(&guest)?;
    let bytes = fs::read(&guest).map_err(|error| error.to_string())?;
    let headers = image["ProgramHeaders"]
        .as_array()
        .ok_or("missing ELF segments")?;
    for (pc, code) in addresses {
        if let Some(location) = image_location(*pc, code, headers, &bytes) {
            locations.insert(*pc, location);
        }
    }
    let disassembly = command_output(
        Command::new("llvm-objdump")
            .args(["-d", "--no-debuginfod"])
            .arg(&guest),
    )?;
    for line in disassembly.lines() {
        if let Some((address, instruction)) = line.trim().split_once(':')
            && let Ok(address) = u64::from_str_radix(address, 16)
            && let Some(location) = locations.get_mut(&address)
        {
            location["disassembly"] = json!(instruction.trim());
        }
    }
    if available(&saved, "symbols.elf", manifest, warnings)? {
        symbolize(&saved.join("symbols.elf"), &mut locations, warnings)?;
    }
    Ok(locations)
}

fn available(
    directory: &Path,
    name: &str,
    manifest: &Value,
    warnings: &mut Vec<String>,
) -> Result<bool, String> {
    let path = directory.join(name);
    if !path.exists() {
        warnings.push(format!(
            "Missing {name}; its address information remains unknown."
        ));
        return Ok(false);
    }
    if manifest["files"][name].as_str() != Some(artifacts::digest(&path)?.as_str()) {
        return Err(format!("artifact checksum mismatch: {}", path.display()));
    }
    Ok(true)
}

fn image_location(pc: u64, code: &str, headers: &[Value], bytes: &[u8]) -> Option<Value> {
    for (index, header) in headers.iter().enumerate() {
        let segment = &header["ProgramHeader"];
        if segment["Type"]["Value"] != 1 || segment["Flags"]["Value"].as_u64()? & 1 == 0 {
            continue;
        }
        let offset = match pc.checked_sub(segment["VirtualAddress"].as_u64()?) {
            Some(offset) => offset,
            None => continue,
        };
        let size = u64::try_from(code.len() / 2).ok()?;
        if offset.checked_add(size)? > segment["FileSize"].as_u64()? {
            continue;
        }
        let start = usize::try_from(segment["Offset"].as_u64()?.checked_add(offset)?).ok()?;
        let end = start.checked_add(usize::try_from(size).ok()?)?;
        let actual = bytes
            .get(start..end)?
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        if actual == code {
            return Some(
                json!({"image": "guest.elf", "segment": index, "offset": format!("0x{offset:x}"), "bytes": code}),
            );
        }
    }
    None
}

fn symbolize(
    path: &Path,
    locations: &mut BTreeMap<u64, Value>,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    let addresses = locations.keys().copied().collect::<Vec<_>>();
    for batch in addresses.chunks(128) {
        let output = Command::new("llvm-symbolizer")
            .args([
                "--output-style=JSON",
                "--inlines",
                "--no-debuginfod",
                "--obj",
            ])
            .arg(path)
            .args(batch.iter().map(|address| format!("0x{address:x}")))
            .output()
            .map_err(|error| error.to_string())?;
        if !output.stderr.is_empty() {
            let message = String::from_utf8_lossy(&output.stderr).into_owned();
            if !warnings.contains(&message) {
                warnings.push(message);
            }
        }
        if !output.status.success() {
            return Err(format!("llvm-symbolizer failed: {}", output.status));
        }
        let frames: Vec<Value> =
            serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;
        if frames.len() != batch.len() {
            return Err("incomplete symbolizer output".to_owned());
        }
        for (address, frame) in batch.iter().zip(frames) {
            if frame["Address"] != format!("0x{address:x}") {
                return Err("unexpected symbolizer address".to_owned());
            }
            locations
                .get_mut(address)
                .ok_or("missing symbolizer address")?["frames"] = frame["Symbol"].clone();
        }
    }
    Ok(())
}
