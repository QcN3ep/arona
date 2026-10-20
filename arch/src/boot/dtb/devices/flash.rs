// SPDX-License-Identifier: MPL-2.0

//! 按需解释 CFI Flash 的总线宽度。

use super::{super::Result, attribute};
use kernel::{HardwareDeviceInfo, HardwareInfo};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FlashInfo {
    pub bank_width: u8,
}

impl FlashInfo {
    pub fn from_device(
        device: &HardwareDeviceInfo,
        hardware: &HardwareInfo,
    ) -> Result<Option<Self>> {
        if !device.has_identity("devicetree", "cfi-flash") {
            return Ok(None);
        }
        let bank_width = attribute::<u32>(hardware, device, "bank-width")?
            .ok_or("flash is missing bank-width")?;
        if !matches!(bank_width, 1 | 2 | 4 | 8) {
            return Err("invalid flash bank width".into());
        }
        Ok(Some(Self {
            bank_width: bank_width as u8,
        }))
    }
}
