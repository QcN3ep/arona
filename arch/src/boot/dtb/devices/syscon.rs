// SPDX-License-Identifier: MPL-2.0

//! 解析系统控制寄存器及关机、重启操作。

use arrayvec::ArrayVec;
use fdt::properties::Compatible;
use kernel::{
    DeviceConfiguration, HardwareDeviceInfo, MAX_DEVICES, PhysRange, RegisterWrite, SysconInfo,
};

use super::super::{
    Dtb, Result,
    nodes::{Node, property},
};
use super::DeviceNode;

pub(super) fn parse(node: Node<'_>, registers: &[PhysRange]) -> Result<SysconInfo> {
    if property::<u32>(node, "reg-shift")?.unwrap_or(0) != 0 {
        return Err("shifted syscon registers are not supported".into());
    }
    let width = property::<u32>(node, "reg-io-width")?.unwrap_or(4);
    if !matches!(width, 1 | 2 | 4)
        || registers.len() != 1
        || registers[0].end - registers[0].start < u64::from(width)
        || !registers[0].start.is_multiple_of(u64::from(width))
    {
        return Err("invalid system controller registers".into());
    }
    Ok(SysconInfo {
        register_width: width as u8,
        endian: super::endian(node)?,
        poweroff: None,
        reboot: None,
    })
}

pub(super) fn operations(
    dtb: &Dtb<'_>,
    nodes: &[DeviceNode<'_>],
    devices: &mut ArrayVec<HardwareDeviceInfo, MAX_DEVICES>,
) -> Result<()> {
    let mut ambiguous = [[false; 2]; MAX_DEVICES];
    let mut walk = dtb.nodes();
    while let Some(node) = walk.next()? {
        let Some(names) = node.property::<Compatible>()? else {
            continue;
        };
        let Some(reboot) = names.into_iter().find_map(|name| match name {
            "syscon-poweroff" => Some(false),
            "syscon-reboot" => Some(true),
            _ => None,
        }) else {
            continue;
        };
        let Ok((index, write)) = operation(dtb, nodes, devices, node) else {
            continue;
        };
        let DeviceConfiguration::Syscon(info) = &mut devices[index].configuration else {
            unreachable!()
        };
        let slot = if reboot {
            &mut info.reboot
        } else {
            &mut info.poweroff
        };
        let ambiguous = &mut ambiguous[index][usize::from(reboot)];
        if *ambiguous || slot.is_some() {
            *ambiguous = true;
            *slot = None;
        } else {
            *slot = Some(write);
        }
    }
    Ok(())
}

fn operation<'a>(
    dtb: &Dtb<'a>,
    nodes: &[DeviceNode<'_>],
    devices: &[HardwareDeviceInfo],
    node: Node<'a>,
) -> Result<(usize, RegisterWrite)> {
    let controller = if let Some(phandle) = property::<u32>(node, "regmap")? {
        dtb.resolve_phandle(phandle)?
    } else {
        node.parent()
            .ok_or("system control operation has no parent")?
    };
    let name = controller.name()?.name;
    let index = nodes
        .iter()
        .position(|device| core::ptr::eq(device.name, name))
        .ok_or("system controller is unsupported")?;
    let device = &devices[index];
    let DeviceConfiguration::Syscon(info) = &device.configuration else {
        return Err("operation does not reference a syscon".into());
    };
    let bits = match info.register_width {
        1 => 0xff,
        2 => 0xffff,
        4 => u32::MAX,
        _ => return Err("unsupported syscon access width".into()),
    };
    // REFERENCE: https://github.com/torvalds/linux/blob/v6.18/drivers/power/reset/syscon-poweroff.c
    let value = property::<u32>(node, "value")?;
    let mask = property::<u32>(node, "mask")?;
    let (mask, value) = match (mask, value) {
        (Some(mask), Some(value)) => (mask, value),
        (None, Some(value)) => (bits, value),
        (Some(legacy_value), None) => (bits, legacy_value),
        (None, None) => return Err("system control operation is missing value and mask".into()),
    };
    let write = RegisterWrite {
        offset: property(node, "offset")?.ok_or("system control operation is missing offset")?,
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
    Ok((index, write))
}
