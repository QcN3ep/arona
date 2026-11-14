// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Supplies explicit system images and measurement boundaries to the collector.

use super::Window;
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
}

impl Input {
    pub(super) fn is_empty(&self) -> bool {
        self.kernel.is_none()
            && self.initramfs.is_none()
            && self.start.is_none()
            && self.end.is_none()
            && self.success.is_none()
            && self.workload.is_none()
            && self.units.is_none()
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

pub(super) fn window(measurement: &Value, symbols: &BTreeMap<&str, u64>) -> Result<Window, String> {
    let address = |key| {
        let value = measurement[key].as_str().ok_or("missing system boundary")?;
        if let Some(hex) = value.strip_prefix("0x") {
            u64::from_str_radix(hex, 16).map_err(|error| error.to_string())
        } else {
            symbols
                .get(value)
                .copied()
                .ok_or_else(|| format!("missing boundary symbol: {value}"))
        }
    };
    let start = address("start")?;
    let end = address("end")?;
    if start == end || start == 0 || end == 0 {
        return Err("system boundaries must be distinct nonzero addresses".to_owned());
    }
    Ok(Window {
        name: "workload",
        start,
        end,
        expected: None,
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
