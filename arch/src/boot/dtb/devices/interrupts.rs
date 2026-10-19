// SPDX-License-Identifier: MPL-2.0

//! 解析平台设备的中断输入并关联设备表中的 PLIC。

use arrayvec::ArrayVec;
use fdt::properties::values::U32List;
use kernel::{HardwareDeviceKind, HardwareInterruptInfo, boot::MAX_INTERRUPTS};

use super::super::{
    Dtb, Result,
    nodes::{Nodes, property},
};

use super::{DeviceNode, instance_id};

pub(super) fn parse(
    dtb: &Dtb<'_>,
    walk: &Nodes<'_>,
    nodes: &[DeviceNode<'_>],
) -> Result<ArrayVec<HardwareInterruptInfo, MAX_INTERRUPTS>> {
    let node = walk.node();
    let mut output = ArrayVec::new();
    if let Some(cells) = property::<U32List<'_>>(node, "interrupts-extended")? {
        let mut cells = cells.iter();
        while let Some(phandle) = cells.next() {
            let line = cells.next().ok_or("invalid interrupts-extended")?;
            output
                .try_push(plic_interrupt(dtb, nodes, phandle, line)?)
                .map_err(|_| "too many interrupts")?;
        }
    } else if let Some(cells) = property::<U32List<'_>>(node, "interrupts")? {
        let mut phandle = None;
        for parent in core::iter::once(node).chain(walk.ancestors()) {
            if let Some(value) = property::<u32>(parent, "interrupt-parent")? {
                phandle = Some(value);
                break;
            }
        }
        let phandle = phandle.ok_or("device is missing interrupt-parent")?;
        for line in cells.iter() {
            output
                .try_push(plic_interrupt(dtb, nodes, phandle, line)?)
                .map_err(|_| "too many interrupts")?;
        }
    }
    Ok(output)
}

fn plic_interrupt(
    dtb: &Dtb<'_>,
    nodes: &[DeviceNode<'_>],
    phandle: u32,
    line: u32,
) -> Result<HardwareInterruptInfo> {
    let node = dtb.resolve_phandle(phandle)?;
    let name = node.name()?.name;
    let index = nodes
        .iter()
        .position(|device| core::ptr::eq(device.name, name))
        .ok_or("interrupt controller is missing from the device table")?;
    if nodes[index].kind != HardwareDeviceKind::RISCV_PLIC
        || property::<u32>(node, "#interrupt-cells")? != Some(1)
        || !node
            .raw_property("interrupt-controller")?
            .is_some_and(|property| property.value.is_empty())
    {
        return Err("unsupported interrupt controller binding".into());
    }
    let max = property::<u32>(node, "riscv,ndev")?.ok_or("PLIC is missing riscv,ndev")?;
    if line == 0 || line > max || max > 1023 {
        return Err("PLIC interrupt line is out of range".into());
    }
    Ok(HardwareInterruptInfo {
        controller: instance_id(index),
        line,
        trigger: None,
        polarity: None,
    })
}
