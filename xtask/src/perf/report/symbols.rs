// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Resolves verified instruction mappings through saved ELF files and LLVM tools.

use super::{
    super::{artifacts, command_output},
    Address, mappings,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path, process::Command};

pub(super) fn resolve(
    directory: &Path,
    manifest: &Value,
    counts: &Value,
    addresses: &[Address],
    warnings: &mut Vec<String>,
) -> Result<BTreeMap<Address, Value>, String> {
    let saved = directory.join("artifacts");
    let mut images = BTreeMap::<String, Vec<(Address, u64, String)>>::new();
    images.insert(String::new(), Vec::new());
    for image in manifest["images"]
        .as_array()
        .ok_or("missing saved images")?
    {
        images.insert(
            image["directory"]
                .as_str()
                .ok_or("missing image directory")?
                .to_owned(),
            Vec::new(),
        );
    }
    for key @ (context, pc, code) in addresses {
        if let Some((link, image, directory)) =
            mappings::relocate(counts, *context, *pc, code.len() / 2, manifest)
        {
            images
                .get_mut(&directory)
                .ok_or("missing mapped image")?
                .push((key.clone(), link, image));
        }
    }
    let mut locations = BTreeMap::new();
    for (relative, addresses) in images {
        let prefix = if relative.is_empty() {
            String::new()
        } else {
            format!("{relative}/")
        };
        verify(&saved, &format!("{prefix}guest.elf"), manifest)?;
        verify(&saved, &format!("{prefix}symbols.elf"), manifest)?;
        locations.extend(resolve_image(&saved.join(relative), &addresses, warnings)?);
    }
    Ok(locations)
}

fn resolve_image(
    saved: &Path,
    addresses: &[(Address, u64, String)],
    warnings: &mut Vec<String>,
) -> Result<BTreeMap<Address, Value>, String> {
    let mut locations = BTreeMap::new();
    if addresses.is_empty() {
        return Ok(locations);
    }
    let guest = saved.join("guest.elf");
    let image = artifacts::inspect(&guest)?;
    let bytes = fs::read(&guest).map_err(|error| error.to_string())?;
    let headers = image["ProgramHeaders"]
        .as_array()
        .ok_or("missing ELF segments")?;
    let mut queries = BTreeMap::<u64, Vec<Address>>::new();
    for (key, link, image) in addresses {
        if let Some(mut location) = image_location(*link, &key.2, headers, &bytes) {
            location["image"] = json!(image);
            location["link_pc"] = json!(format!("0x{link:x}"));
            locations.insert(key.clone(), location);
            queries.entry(*link).or_default().push(key.clone());
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
            && let Some(keys) = queries.get(&address)
        {
            for key in keys {
                locations
                    .get_mut(key)
                    .ok_or("missing instruction location")?["disassembly"] =
                    json!(instruction.trim());
            }
        }
    }
    symbolize(
        &saved.join("symbols.elf"),
        &queries,
        &mut locations,
        warnings,
    )?;
    Ok(locations)
}

fn verify(directory: &Path, name: &str, manifest: &Value) -> Result<(), String> {
    let path = directory.join(name);
    if manifest["files"][name].as_str() != Some(artifacts::digest(&path)?.as_str()) {
        return Err(format!("artifact checksum mismatch: {}", path.display()));
    }
    Ok(())
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
    queries: &BTreeMap<u64, Vec<Address>>,
    locations: &mut BTreeMap<Address, Value>,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    let addresses = queries.keys().copied().collect::<Vec<_>>();
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
            for key in queries.get(address).ok_or("missing symbolizer query")? {
                locations.get_mut(key).ok_or("missing symbolizer address")?["frames"] =
                    frame["Symbol"].clone();
            }
        }
    }
    Ok(())
}
