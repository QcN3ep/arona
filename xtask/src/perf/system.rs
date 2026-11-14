// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Supplies explicit system images and measurement boundaries to the collector.

use super::{Window, artifacts};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::PathBuf};

#[derive(Default)]
pub(super) struct Input {
    pub kernel: Option<PathBuf>,
    pub initramfs: Option<PathBuf>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub success: Option<String>,
    pub workload: Option<String>,
    pub units: Option<u64>,
    pub images: Vec<String>,
}

impl Input {
    pub(super) fn set(&mut self, argument: &str, value: String) -> Result<(), String> {
        match argument {
            "--kernel" => self.kernel = Some(value.into()),
            "--initramfs" => self.initramfs = Some(value.into()),
            "--start" => self.start = Some(value),
            "--end" => self.end = Some(value),
            "--success" => self.success = Some(value),
            "--workload" => self.workload = Some(value),
            "--units" => self.units = Some(value.parse().map_err(|_| "invalid work units")?),
            "--image" => self.images.push(value),
            _ => return Err(format!("unknown system option: {argument}")),
        }
        Ok(())
    }

    pub(super) fn is_empty(&self) -> bool {
        self.kernel.is_none()
            && self.initramfs.is_none()
            && self.start.is_none()
            && self.end.is_none()
            && self.success.is_none()
            && self.workload.is_none()
            && self.units.is_none()
            && self.images.is_empty()
    }

    pub(super) fn measurement(&self) -> Result<Value, String> {
        let required = |value: &Option<String>, name| {
            value
                .as_deref()
                .filter(|value| !value.is_empty() && !value.contains(['\n', '\r']))
                .ok_or_else(|| format!("system collection requires --{name}"))
                .map(str::to_owned)
        };
        Ok(json!({
            "start": required(&self.start, "start")?,
            "end": required(&self.end, "end")?,
            "success": required(&self.success, "success")?,
            "workload": required(&self.workload, "workload")?,
            "units": self.units.filter(|units| *units > 0).ok_or("system collection requires positive --units")?,
        }))
    }

    pub(super) fn preserve(&self, directory: &std::path::Path) -> Result<(), String> {
        for (path, option, name) in [
            (&self.kernel, "kernel", "symbols.elf"),
            (&self.initramfs, "initramfs", "initramfs.cpio"),
        ] {
            let path = path
                .as_ref()
                .ok_or_else(|| format!("system collection requires --{option}"))?;
            fs::copy(path, directory.join(name))
                .map_err(|error| format!("{}: {error}", path.display()))?;
        }
        Ok(())
    }
}

pub(super) fn preserve_images(
    input: &Input,
    directory: &std::path::Path,
) -> Result<Vec<Value>, String> {
    let archive = fs::read(directory.join("initramfs.cpio")).map_err(|error| error.to_string())?;
    let mut images = Vec::new();
    for (index, spec) in input.images.iter().enumerate() {
        let (name, path) = spec
            .split_once('=')
            .ok_or("--image requires ARCHIVE_PATH=ELF")?;
        if name.is_empty() || images.iter().any(|image: &Value| image["name"] == name) {
            return Err("empty or duplicate component image name".to_owned());
        }
        let saved = format!("images/{index}");
        let destination = directory.join(&saved);
        fs::create_dir_all(&destination).map_err(|error| error.to_string())?;
        fs::copy(path, destination.join("symbols.elf"))
            .map_err(|error| format!("{path}: {error}"))?;
        fs::write(destination.join("guest.elf"), archive_file(&archive, name)?)
            .map_err(|error| error.to_string())?;
        images.push(json!({"name":name,"directory":saved}));
    }
    Ok(images)
}

pub(super) fn image_files(manifest: &Value) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    for (index, image) in manifest["images"]
        .as_array()
        .ok_or("missing saved component images")?
        .iter()
        .enumerate()
    {
        let directory = format!("images/{index}");
        if image["directory"] != directory || image["name"].as_str().is_none_or(str::is_empty) {
            return Err("invalid saved component image".to_owned());
        }
        for name in ["symbols.elf", "guest.elf"] {
            files.push(format!("{directory}/{name}"));
        }
    }
    Ok(files)
}

pub(super) fn validate_images(
    target: super::Target,
    directory: &std::path::Path,
    manifest: &Value,
) -> Result<(), String> {
    let archive = fs::read(directory.join("initramfs.cpio")).map_err(|error| error.to_string())?;
    for image in manifest["images"]
        .as_array()
        .ok_or("missing saved component images")?
    {
        let path = directory.join(
            image["directory"]
                .as_str()
                .ok_or("missing image directory")?,
        );
        let name = image["name"].as_str().ok_or("missing image name")?;
        if fs::read(path.join("guest.elf")).map_err(|error| error.to_string())?
            != archive_file(&archive, name)?
        {
            return Err(format!("saved image differs from initramfs: {name}"));
        }
        artifacts::validate_images(target, &path, "component")?;
    }
    Ok(())
}

fn archive_file<'a>(archive: &'a [u8], name: &str) -> Result<&'a [u8], String> {
    let mut offset = 0usize;
    loop {
        let header = archive
            .get(offset..offset.checked_add(110).ok_or("CPIO offset overflow")?)
            .ok_or("truncated CPIO header")?;
        if &header[..6] != b"070701" {
            return Err("unsupported CPIO header".to_owned());
        }
        let field = |start| {
            usize::from_str_radix(
                std::str::from_utf8(&header[start..start + 8])
                    .map_err(|error| error.to_string())?,
                16,
            )
            .map_err(|error| error.to_string())
        };
        let size = field(54)?;
        let namesize = field(94)?;
        offset += 110;
        let end = offset.checked_add(namesize).ok_or("CPIO name overflow")?;
        let entry = archive
            .get(offset..end)
            .and_then(|value| value.strip_suffix(&[0]))
            .ok_or("invalid CPIO name")?;
        offset = end.checked_add(3).ok_or("CPIO alignment overflow")? & !3;
        let end = offset.checked_add(size).ok_or("CPIO file overflow")?;
        let data = archive.get(offset..end).ok_or("truncated CPIO file")?;
        if entry == name.as_bytes() {
            return Ok(data);
        }
        if entry == b"TRAILER!!!" {
            return Err(format!("image is absent from initramfs: {name}"));
        }
        offset = end.checked_add(3).ok_or("CPIO alignment overflow")? & !3;
    }
}

pub(super) fn window(
    directory: &std::path::Path,
    manifest: &Value,
    symbols: &BTreeMap<&str, u64>,
) -> Result<Window, String> {
    let measurement = &manifest["measurement"];
    let address = |key| {
        let value = measurement[key].as_str().ok_or("missing system boundary")?;
        let (value, image) = if let Some((name, symbol)) = value.split_once(':') {
            let index = manifest["images"]
                .as_array()
                .ok_or("missing saved images")?
                .iter()
                .position(|image| image["name"] == name)
                .ok_or_else(|| format!("boundary image was not supplied: {name}"))?;
            (symbol, Some(index))
        } else {
            (value, None)
        };
        if let Some(hex) = value.strip_prefix("0x") {
            Ok((
                u64::from_str_radix(hex, 16).map_err(|error| error.to_string())?,
                image,
            ))
        } else if let Some(index) = image {
            let output = super::command_output(
                std::process::Command::new("llvm-nm")
                    .args(["--format=posix", "--defined-only", "--no-demangle"])
                    .arg(directory.join(format!("images/{index}/symbols.elf"))),
            )?;
            let pc = output
                .lines()
                .find_map(|line| {
                    let mut fields = line.split_whitespace();
                    if fields.next()? != value {
                        return None;
                    }
                    fields.next()?;
                    u64::from_str_radix(fields.next()?, 16).ok()
                })
                .ok_or_else(|| format!("missing boundary symbol: {value}"))?;
            Ok((pc, image))
        } else {
            symbols
                .get(value)
                .copied()
                .map(|pc| (pc, image))
                .ok_or_else(|| format!("missing boundary symbol: {value}"))
        }
    };
    let (start, start_image) = address("start")?;
    let (end, end_image) = address("end")?;
    if (start == end && start_image == end_image) || start == 0 || end == 0 {
        return Err("system boundaries must be distinct nonzero addresses".to_owned());
    }
    Ok(Window {
        name: "workload",
        start,
        end,
        expected: None,
        start_image,
        end_image,
    })
}

pub(super) fn validate_measurement(measurement: &Value) -> Result<(), String> {
    for key in ["start", "end", "success", "workload"] {
        if measurement[key].as_str().is_none_or(str::is_empty) {
            return Err(format!("missing system measurement {key}"));
        }
    }
    if measurement["units"].as_u64().is_none_or(|units| units == 0) {
        return Err("invalid system work units".to_owned());
    }
    Ok(())
}

pub(super) fn files(target: super::Target) -> Vec<&'static str> {
    let mut files = vec!["symbols.elf", "guest.elf", "initramfs.cpio"];
    if matches!(target, super::Target::Riscv64) {
        files.push("firmware.bin");
    }
    files
}
