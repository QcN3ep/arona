// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Parse platform device interrupt inputs and associate them with controllers in the device table.

use arrayvec::ArrayVec;
use fdt::properties::values::U32List;
use kernel::{
    CpuInterruptRoute, CpuInterruptTarget, HardwareDeviceInfo, HardwareDeviceInstanceId,
    HardwareDeviceLocation, HardwareInfo, HardwareInterruptInfo, HardwareInterruptRoute,
    InterruptPolarity, InterruptTrigger, MAX_CPU_INTERRUPT_ROUTES, boot::MAX_INTERRUPTS,
    memory::PhysRange,
};

use super::super::{
    Dtb, Result,
    nodes::{Node, Nodes, integer, property, registers},
    range,
};

use super::{DeviceNode, find_device, instance_id, node_index};
use core::ops::Range;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Controller {
    RiscvPlic,
    RiscvClint,
    LoongarchEiointc,
    LoongarchPchPic,
    LoongarchPchMsi,
}

fn kind(node: Node<'_>) -> Result<Option<Controller>> {
    Ok(node
        .property::<fdt::properties::Compatible>()?
        .and_then(|names| {
            names.into_iter().find_map(|name| match name {
                "riscv,plic0" | "sifive,plic-1.0.0" => Some(Controller::RiscvPlic),
                "riscv,clint0" | "sifive,clint0" => Some(Controller::RiscvClint),
                "loongson,ls2k2000-eiointc" => Some(Controller::LoongarchEiointc),
                "loongson,pch-pic-1.0" => Some(Controller::LoongarchPchPic),
                "loongson,pch-msi-1.0" => Some(Controller::LoongarchPchMsi),
                _ => None,
            })
        }))
}

pub(super) fn is_msi(node: Node<'_>) -> Result<bool> {
    Ok(kind(node)? == Some(Controller::LoongarchPchMsi))
}

pub(super) fn location(walk: &Nodes<'_>) -> Result<Option<HardwareDeviceLocation>> {
    if kind(walk.node())? != Some(Controller::LoongarchEiointc) {
        return Ok(None);
    }
    let mut entries = registers(walk.node())?.iter::<u64, u64>();
    let entry = entries.next().ok_or("EIOINTC is missing reg")??;
    if entries.next().is_some() {
        return Err("EIOINTC has multiple register ranges".into());
    }
    // REFERENCE: https://docs.kernel.org/arch/loongarch/irq-chip-model.html
    let registers = range(entry.address, entry.len)?;
    let start = registers
        .start
        .try_into()
        .map_err(|_| "IOCSR address overflow")?;
    let end = registers
        .end
        .try_into()
        .map_err(|_| "IOCSR address overflow")?;
    Ok(Some(HardwareDeviceLocation::Iocsr {
        registers: start..end,
    }))
}

pub(super) fn resources(
    dtb: &Dtb<'_>,
    walk: &Nodes<'_>,
    nodes: &[DeviceNode<'_>],
    device: &mut HardwareDeviceInfo,
    cpu_interrupts: &mut ArrayVec<CpuInterruptRoute, MAX_CPU_INTERRUPT_ROUTES>,
) -> Result<()> {
    let Some(kind) = kind(walk.node())? else {
        device.interrupts = parse(dtb, walk, nodes)?;
        return Ok(());
    };
    match kind {
        Controller::RiscvPlic | Controller::RiscvClint | Controller::LoongarchEiointc => {
            cpu_routes(dtb, walk, device.instance_id, cpu_interrupts)?;
        }
        Controller::LoongarchPchPic => device.interrupts = parse(dtb, walk, nodes)?,
        Controller::LoongarchPchMsi => {}
    }
    if matches!(
        kind,
        Controller::RiscvPlic | Controller::LoongarchEiointc | Controller::LoongarchPchPic
    ) {
        device.interrupt_lines = Some(lines(walk.node(), kind)?);
    }
    if kind == Controller::LoongarchPchMsi {
        device.msi_vectors = Some(vectors(walk.node(), &device.mmio_regions)?);
    }
    if matches!(
        kind,
        Controller::LoongarchPchPic | Controller::LoongarchPchMsi
    ) {
        device.interrupt_route = Some(cascade(dtb, walk, nodes, kind)?);
    }
    Ok(())
}

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
    kind: Controller,
) -> Result<HardwareInterruptRoute> {
    let (index, controller_node) = controller(dtb, nodes, parent(walk)?)?;
    if self::kind(controller_node)? != Some(Controller::LoongarchEiointc) {
        return Err("unsupported PCH PIC parent controller".into());
    }
    let base_line = if kind == Controller::LoongarchPchMsi {
        0
    } else {
        property::<u32>(walk.node(), "loongson,pic-base-vec")?
            .filter(|&base| base <= 256 - 64)
            .ok_or("PCH PIC base vector is missing or out of range")?
    };
    Ok(HardwareInterruptRoute {
        controller: instance_id(index),
        base_line,
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
    let index =
        node_index(nodes, node)?.ok_or("interrupt controller is missing from the device table")?;
    let cells = match kind(node)? {
        Some(Controller::RiscvPlic) | Some(Controller::LoongarchEiointc) => 1,
        Some(Controller::LoongarchPchPic) => 2,
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
    let (trigger, polarity) = match kind(node)? {
        Some(Controller::RiscvPlic) => {
            let max = property::<u32>(node, "riscv,ndev")?.ok_or("PLIC is missing riscv,ndev")?;
            if line == 0 || line > max || max > 1023 {
                return Err("PLIC interrupt line is out of range".into());
            }
            (None, None)
        }
        Some(Controller::LoongarchEiointc) if line < 256 => (None, None),
        Some(Controller::LoongarchPchPic) if line < 64 => {
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

pub(super) fn vectors(node: Node<'_>, registers: &[PhysRange]) -> Result<Range<u32>> {
    let start =
        property::<u32>(node, "loongson,msi-base-vec")?.ok_or("MSI is missing base vector")?;
    let count =
        property::<u32>(node, "loongson,msi-num-vecs")?.ok_or("MSI is missing vector count")?;
    let end = start
        .checked_add(count)
        .ok_or("MSI vector range overflow")?;
    if count == 0 || end > 256 || registers.first().is_none_or(|reg| reg.end - reg.start < 4) {
        return Err("invalid MSI controller configuration".into());
    }
    Ok(start..end)
}

pub(super) fn lines(node: Node<'_>, kind: Controller) -> Result<Range<u32>> {
    let lines = match kind {
        Controller::RiscvPlic => {
            let count = property::<u32>(node, "riscv,ndev")?.ok_or("PLIC is missing riscv,ndev")?;
            if !(1..=1023).contains(&count) {
                return Err("invalid PLIC source count".into());
            }
            1..count + 1
        }
        Controller::LoongarchEiointc => 0..256,
        Controller::LoongarchPchPic => 0..64,
        _ => unreachable!(),
    };
    let expected = if kind == Controller::LoongarchPchPic {
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
    Ok(lines)
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

pub(super) fn validate(hardware: &HardwareInfo) -> Result<()> {
    cpu_targets(hardware)?;
    for (index, device) in hardware.devices.iter().enumerate() {
        for (position, interrupt) in device.interrupts.iter().enumerate() {
            let current = source(hardware, *interrupt)?;
            for other in hardware.devices[..index]
                .iter()
                .flat_map(|device| &device.interrupts)
                .chain(&device.interrupts[..position])
            {
                if source(hardware, *other)? == current {
                    return Err("shared IRQs are not supported".into());
                }
            }
        }
    }
    Ok(())
}

pub(super) fn source(
    hardware: &HardwareInfo,
    interrupt: HardwareInterruptInfo,
) -> Result<(HardwareDeviceInstanceId, u32)> {
    let controller = find_device(hardware, interrupt.controller)?;
    let lines = controller
        .interrupt_lines
        .as_ref()
        .ok_or("invalid interrupt controller")?;
    if !lines.contains(&interrupt.line) {
        return Err("interrupt line is out of range".into());
    }
    if let Some(route) = controller.interrupt_route {
        let parent = find_device(hardware, route.controller)?;
        let line = route
            .base_line
            .checked_add(interrupt.line)
            .filter(|line| {
                parent
                    .interrupt_lines
                    .as_ref()
                    .is_some_and(|lines| lines.contains(line))
            })
            .ok_or("interrupt route is out of range")?;
        return Ok((route.controller, line));
    }
    Ok((interrupt.controller, interrupt.line))
}

fn cpu_targets(hardware: &HardwareInfo) -> Result<()> {
    for route in &hardware.cpu_interrupts {
        let controller = find_device(hardware, route.controller)?;
        let valid = if controller.has_identity("devicetree", "riscv,plic0")
            || controller.has_identity("devicetree", "sifive,plic-1.0.0")
        {
            matches!(route.target, CpuInterruptTarget::Cpu(_)) && matches!(route.line, 9 | 11)
        } else if controller.has_identity("devicetree", "riscv,clint0")
            || controller.has_identity("devicetree", "sifive,clint0")
        {
            matches!(route.target, CpuInterruptTarget::Cpu(_)) && matches!(route.line, 3 | 7)
        } else {
            controller.has_identity("devicetree", "loongson,ls2k2000-eiointc")
                && route.target == CpuInterruptTarget::AllCpus
                && route.line < 14
        };
        if !valid {
            return Err("invalid CPU interrupt route".into());
        }
        if let CpuInterruptTarget::Cpu(id) = route.target
            && !hardware.cpus.iter().any(|cpu| cpu.hardware_id == id)
        {
            return Err("interrupt targets an unknown CPU".into());
        }
    }
    Ok(())
}
