// SPDX-License-Identifier: MPL-2.0

//! 解析平台设备的中断输入并关联设备表中的控制器。

use arrayvec::ArrayVec;
use fdt::properties::values::U32List;
use kernel::{
    CpuInterruptRoute, CpuInterruptTarget, DeviceConfiguration, HardwareDeviceInstanceId,
    HardwareDeviceKind, HardwareInterruptInfo, HardwareInterruptRoute, InterruptPolarity,
    InterruptTrigger, MAX_CPU_INTERRUPT_ROUTES, boot::MAX_INTERRUPTS,
};

use super::super::{
    Dtb, Result,
    nodes::{Node, Nodes, integer, property},
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
            output
                .try_push(interrupt(dtb, nodes, phandle, &mut cells)?)
                .map_err(|_| "too many interrupts")?;
        }
    } else if let Some(cells) = property::<U32List<'_>>(node, "interrupts")? {
        let phandle = parent(walk)?;
        let mut cells = cells.iter().peekable();
        while cells.peek().is_some() {
            output
                .try_push(interrupt(dtb, nodes, phandle, &mut cells)?)
                .map_err(|_| "too many interrupts")?;
        }
    }
    Ok(output)
}

pub(super) fn cascade(
    dtb: &Dtb<'_>,
    walk: &Nodes<'_>,
    nodes: &[DeviceNode<'_>],
    kind: HardwareDeviceKind,
) -> Result<HardwareInterruptRoute> {
    let (index, _) = controller(dtb, nodes, parent(walk)?)?;
    if nodes[index].kind != HardwareDeviceKind::LOONGARCH_EIOINTC {
        return Err("unsupported PCH PIC parent controller".into());
    }
    Ok(HardwareInterruptRoute {
        controller: instance_id(index),
        base_line: if kind == HardwareDeviceKind::LOONGARCH_PCH_MSI {
            0
        } else {
            property::<u32>(walk.node(), "loongson,pic-base-vec")?
                .ok_or("PCH PIC is missing loongson,pic-base-vec")?
        },
    })
}

pub(super) fn parent(walk: &Nodes<'_>) -> Result<u32> {
    for node in core::iter::once(walk.node()).chain(walk.ancestors()) {
        if let Some(phandle) = property::<u32>(node, "interrupt-parent")? {
            return Ok(phandle);
        }
    }
    Err("device is missing interrupt-parent".into())
}

pub(super) fn controller<'a>(
    dtb: &Dtb<'a>,
    nodes: &[DeviceNode<'_>],
    phandle: u32,
) -> Result<(usize, Node<'a>)> {
    let node = dtb.resolve_phandle(phandle)?;
    let name = node.name()?.name;
    let index = nodes
        .iter()
        .position(|device| core::ptr::eq(device.name, name))
        .ok_or("interrupt controller is missing from the device table")?;
    let cells = match nodes[index].kind {
        HardwareDeviceKind::RISCV_PLIC | HardwareDeviceKind::LOONGARCH_EIOINTC => 1,
        HardwareDeviceKind::LOONGARCH_PCH_PIC => 2,
        _ => return Err("unsupported interrupt controller binding".into()),
    };
    if property::<u32>(node, "#interrupt-cells")? != Some(cells)
        || !node
            .raw_property("interrupt-controller")?
            .is_some_and(|property| property.value.is_empty())
    {
        return Err("unsupported interrupt controller binding".into());
    }
    Ok((index, node))
}

pub(super) fn interrupt(
    dtb: &Dtb<'_>,
    nodes: &[DeviceNode<'_>],
    phandle: u32,
    cells: &mut impl Iterator<Item = u32>,
) -> Result<HardwareInterruptInfo> {
    let (index, node) = controller(dtb, nodes, phandle)?;
    let line = cells.next().ok_or("truncated interrupt specifier")?;
    let (trigger, polarity) = match nodes[index].kind {
        HardwareDeviceKind::RISCV_PLIC => {
            let max = property::<u32>(node, "riscv,ndev")?.ok_or("PLIC is missing riscv,ndev")?;
            if line == 0 || line > max || max > 1023 {
                return Err("PLIC interrupt line is out of range".into());
            }
            (None, None)
        }
        HardwareDeviceKind::LOONGARCH_EIOINTC if line < 256 => (None, None),
        HardwareDeviceKind::LOONGARCH_PCH_PIC if line < 64 => {
            // REFERENCE: https://github.com/torvalds/linux/blob/v6.18/include/dt-bindings/interrupt-controller/irq.h
            let (trigger, polarity) = match cells.next() {
                Some(1) => (InterruptTrigger::Edge, InterruptPolarity::ActiveHigh),
                Some(2) => (InterruptTrigger::Edge, InterruptPolarity::ActiveLow),
                Some(4) => (InterruptTrigger::Level, InterruptPolarity::ActiveHigh),
                Some(8) => (InterruptTrigger::Level, InterruptPolarity::ActiveLow),
                _ => return Err("unsupported PCH PIC interrupt flags".into()),
            };
            (Some(trigger), Some(polarity))
        }
        _ => return Err("interrupt line is out of range".into()),
    };
    Ok(HardwareInterruptInfo {
        controller: instance_id(index),
        line,
        trigger,
        polarity,
    })
}

pub(super) fn configuration(
    node: Node<'_>,
    kind: HardwareDeviceKind,
) -> Result<DeviceConfiguration> {
    if kind == HardwareDeviceKind::LOONGARCH_PCH_MSI {
        let start =
            property::<u32>(node, "loongson,msi-base-vec")?.ok_or("MSI is missing base vector")?;
        let count =
            property::<u32>(node, "loongson,msi-num-vecs")?.ok_or("MSI is missing vector count")?;
        let end = start
            .checked_add(count)
            .ok_or("MSI vector range overflow")?;
        return Ok(DeviceConfiguration::Msi {
            vectors: start..end,
        });
    }
    let lines = match kind {
        HardwareDeviceKind::RISCV_PLIC => {
            let count = property::<u32>(node, "riscv,ndev")?.ok_or("PLIC is missing riscv,ndev")?;
            1..count.checked_add(1).ok_or("PLIC source range overflow")?
        }
        HardwareDeviceKind::LOONGARCH_EIOINTC => 0..256,
        HardwareDeviceKind::LOONGARCH_PCH_PIC => 0..64,
        _ => unreachable!(),
    };
    let expected = if kind == HardwareDeviceKind::LOONGARCH_PCH_PIC {
        2
    } else {
        1
    };
    if property::<u32>(node, "#interrupt-cells")? != Some(expected)
        || !node
            .raw_property("interrupt-controller")?
            .is_some_and(|p| p.value.is_empty())
    {
        return Err("invalid interrupt controller binding".into());
    }
    Ok(DeviceConfiguration::InterruptController { lines })
}

pub(super) fn cpu_routes(
    dtb: &Dtb<'_>,
    walk: &Nodes<'_>,
    controller: HardwareDeviceInstanceId,
    output: &mut ArrayVec<CpuInterruptRoute, MAX_CPU_INTERRUPT_ROUTES>,
) -> Result<()> {
    let node = walk.node();
    if let Some(cells) = property::<U32List<'_>>(node, "interrupts-extended")? {
        let mut cells = cells.iter();
        let mut index = 0;
        while let Some(phandle) = cells.next() {
            let target = dtb.resolve_phandle(phandle)?;
            let line = cells.next().ok_or("truncated CPU interrupt specifier")?;
            let compatible = target
                .property::<fdt::properties::Compatible>()?
                .is_some_and(|names| names.into_iter().any(|name| name == "riscv,cpu-intc"));
            if !compatible
                || property::<u32>(target, "#interrupt-cells")? != Some(1)
                || target.raw_property("interrupt-controller")?.is_none()
            {
                return Err("unsupported CPU interrupt controller".into());
            }
            let cpu = target
                .parent()
                .ok_or("CPU interrupt controller has no CPU")?;
            if property::<&str>(cpu, "device_type")? != Some("cpu") {
                return Err("local interrupt controller does not belong to a CPU".into());
            }
            let id = integer(cpu, "reg")?.ok_or("CPU has no hardware ID")?;
            if line != u32::MAX {
                output
                    .try_push(CpuInterruptRoute {
                        controller,
                        output: index,
                        target: CpuInterruptTarget::Cpu(id),
                        line,
                    })
                    .map_err(|_| "too many CPU interrupt routes")?;
            }
            index += 1;
        }
    } else if let Some(cells) = property::<U32List<'_>>(node, "interrupts")? {
        let target = dtb.resolve_phandle(parent(walk)?)?;
        if !target
            .property::<fdt::properties::Compatible>()?
            .is_some_and(|names| {
                names
                    .into_iter()
                    .any(|name| name == "loongson,cpu-interrupt-controller")
            })
            || property::<u32>(target, "#interrupt-cells")? != Some(1)
        {
            return Err("unsupported CPU interrupt controller".into());
        }
        for (index, line) in cells.iter().enumerate() {
            output
                .try_push(CpuInterruptRoute {
                    controller,
                    output: index as u32,
                    target: CpuInterruptTarget::AllCpus,
                    line,
                })
                .map_err(|_| "too many CPU interrupt routes")?;
        }
    }
    Ok(())
}
