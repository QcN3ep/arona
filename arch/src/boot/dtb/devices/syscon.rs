// SPDX-License-Identifier: MPL-2.0

//! 按需解释系统控制寄存器及关机、重启操作。

use kernel::{HardwareDeviceInfo, HardwareInfo};

use super::super::Result;
use super::{RegisterEndian, attribute, resolve};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SysconInfo {
    pub register_width: u8,
    pub endian: RegisterEndian,
    pub poweroff: Option<RegisterWrite>,
    pub reboot: Option<RegisterWrite>,
}

/// 以 `(old & !mask) | (value & mask)` 更新寄存器。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegisterWrite {
    pub offset: u32,
    pub mask: u32,
    pub value: u32,
}

impl SysconInfo {
    pub fn from_device(
        device: &HardwareDeviceInfo,
        hardware: &HardwareInfo,
    ) -> Result<Option<Self>> {
        if !device.has_identity("devicetree", "syscon") {
            return Ok(None);
        }
        let mut info = parse(hardware, device)?;
        let mut ambiguous = [false; 2];
        for node in &hardware.devices {
            let reboot = if node.has_identity("devicetree", "syscon-reboot") {
                true
            } else if node.has_identity("devicetree", "syscon-poweroff") {
                false
            } else {
                continue;
            };
            let Ok((controller, write)) = operation(hardware, node) else {
                continue;
            };
            if controller != device.instance_id {
                continue;
            }
            let slot = if reboot {
                &mut info.reboot
            } else {
                &mut info.poweroff
            };
            let ambiguous = &mut ambiguous[usize::from(reboot)];
            if *ambiguous || slot.is_some() {
                *ambiguous = true;
                *slot = None;
            } else {
                *slot = Some(write);
            }
        }
        Ok(Some(info))
    }
}

fn parse(hardware: &HardwareInfo, device: &HardwareDeviceInfo) -> Result<SysconInfo> {
    let registers = &device.mmio_regions;
    if attribute::<u32>(hardware, device, "reg-shift")?.unwrap_or(0) != 0 {
        return Err("shifted syscon registers are not supported".into());
    }
    let width = attribute::<u32>(hardware, device, "reg-io-width")?.unwrap_or(4);
    if !matches!(width, 1 | 2 | 4)
        || registers.len() != 1
        || registers[0].end - registers[0].start < u64::from(width)
        || !registers[0].start.is_multiple_of(u64::from(width))
    {
        return Err("invalid system controller registers".into());
    }
    Ok(SysconInfo {
        register_width: width as u8,
        endian: super::endian(hardware, device)?,
        poweroff: None,
        reboot: None,
    })
}

fn operation(
    hardware: &HardwareInfo,
    node: &HardwareDeviceInfo,
) -> Result<(kernel::HardwareDeviceInstanceId, RegisterWrite)> {
    let device = if let Some(phandle) = attribute::<u32>(hardware, node, "regmap")? {
        resolve(hardware, phandle)?
    } else {
        hardware
            .devices
            .iter()
            .find(|device| Some(device.instance_id) == node.parent)
            .ok_or("system control operation has no parent")?
    };
    if !device.has_identity("devicetree", "syscon") {
        return Err("operation does not reference a syscon".into());
    }
    let info = parse(hardware, device)?;
    let bits = match info.register_width {
        1 => 0xff,
        2 => 0xffff,
        4 => u32::MAX,
        _ => return Err("unsupported syscon access width".into()),
    };
    // REFERENCE: https://github.com/torvalds/linux/blob/v6.18/drivers/power/reset/syscon-poweroff.c
    let value = attribute::<u32>(hardware, node, "value")?;
    let mask = attribute::<u32>(hardware, node, "mask")?;
    let (mask, value) = match (mask, value) {
        (Some(mask), Some(value)) => (mask, value),
        (None, Some(value)) => (bits, value),
        (Some(legacy_value), None) => (bits, legacy_value),
        (None, None) => return Err("system control operation is missing value and mask".into()),
    };
    let write = RegisterWrite {
        offset: attribute(hardware, node, "offset")?
            .ok_or("system control operation is missing offset")?,
        mask,
        value,
    };
    let registers = device
        .mmio_regions
        .first()
        .ok_or("syscon has no registers")?;
    if u64::from(write.offset) + u64::from(info.register_width)
        > registers.end.saturating_sub(registers.start)
        || !write.offset.is_multiple_of(u32::from(info.register_width))
        || write.mask == 0
        || write.mask & !bits != 0
        || write.value & !bits != 0
    {
        return Err("invalid system control register operation".into());
    }
    Ok((device.instance_id, write))
}
