// SPDX-License-Identifier: MPL-2.0

//! Parse the address windows and interrupt routing of the generic ECAM host bridge.

use super::{DeviceNode, instance_id, interrupts};
use arrayvec::ArrayVec;
use fdt::properties::{
    pci::{PciAddress, PciAddressSpace as FdtAddressSpace},
    values::U32List,
};
use kernel::{
    HardwareDeviceLocation, HardwareInfo, PciAddressSpace, PciInfo, PciInterruptRoute, PciMsiMap,
    PciWindow,
};

use super::super::{
    Dtb, Result,
    nodes::{Nodes, cell_sizes, property, ranges, registers},
    range,
};

use super::PAGE_SIZE;

pub(super) fn parse(walk: &Nodes<'_>) -> Result<HardwareDeviceLocation> {
    let node = walk.node();
    let cells = cell_sizes(node)?;
    if (cells.address_cells, cells.size_cells) != (3, 2) {
        return Err("unsupported PCI cell sizes".into());
    }
    let mut entries = registers(node)?.iter::<u64, u64>();
    let ecam = entries.next().ok_or("PCI host bridge is missing ECAM")??;
    if entries.next().is_some() {
        return Err("PCI host bridge has multiple ECAM ranges".into());
    }
    let ecam = walk.translate(range(ecam.address, ecam.len)?)?;
    let bus_range = property::<U32List<'_>>(node, "bus-range")?
        .ok_or("PCI host bridge is missing bus-range")?;
    let mut buses = bus_range.iter();
    let (Some(first), Some(last), None) = (buses.next(), buses.next(), buses.next()) else {
        return Err("PCI bus-range must contain two cells".into());
    };
    if first > last || last > 255 {
        return Err("PCI bus-range is out of bounds".into());
    }
    if ecam.end - ecam.start < u64::from(last - first + 1) << 20 {
        return Err("ECAM does not cover bus-range".into());
    }
    if !ecam.start.is_multiple_of(PAGE_SIZE) || !ecam.end.is_multiple_of(PAGE_SIZE) {
        return Err("ECAM is not page-aligned".into());
    }
    let segment = property::<u32>(node, "linux,pci-domain")?
        .unwrap_or(0)
        .try_into()
        .map_err(|_| "PCI segment is out of range")?;
    let mut windows = ArrayVec::new();
    for entry in ranges(node)?
        .into_iter()
        .flat_map(|ranges| ranges.iter::<PciAddress, u64, u64>())
    {
        let entry = entry?;
        let space = match entry.child_bus_address.hi.address_space() {
            FdtAddressSpace::Io => continue,
            FdtAddressSpace::Memory32 => PciAddressSpace::Memory32,
            FdtAddressSpace::Memory64 => PciAddressSpace::Memory64,
            _ => return Err("PCI ranges contains configuration space".into()),
        };
        let child =
            u64::from(entry.child_bus_address.mid) << 32 | u64::from(entry.child_bus_address.lo);
        let window = PciWindow {
            bus_address: child,
            cpu_range: walk.translate(range(entry.parent_bus_address, entry.len)?)?,
            space,
            prefetchable: entry.child_bus_address.hi.prefetchable(),
        };
        let end = child
            .checked_add(entry.len)
            .ok_or("PCI bus window overflow")?;
        if !child.is_multiple_of(PAGE_SIZE)
            || !window.cpu_range.start.is_multiple_of(PAGE_SIZE)
            || !window.cpu_range.end.is_multiple_of(PAGE_SIZE)
        {
            return Err("PCI memory window is not page-aligned".into());
        }
        if space == PciAddressSpace::Memory32 && end > 1_u64 << 32 {
            return Err("PCI window exceeds its address space".into());
        }
        if windows.iter().any(|other: &PciWindow| {
            child < other.bus_address + (other.cpu_range.end - other.cpu_range.start)
                && other.bus_address < end
        }) {
            return Err("overlapping PCI bus windows".into());
        }
        windows
            .try_push(window)
            .map_err(|_| "too many PCI windows")?;
    }
    Ok(HardwareDeviceLocation::PciHostBridge {
        segment,
        first_bus: first as u8,
        last_bus: last as u8,
        ecam,
        windows,
        iommu: None,
    })
}

pub(super) fn routes(
    dtb: &Dtb<'_>,
    walk: &Nodes<'_>,
    index: usize,
    nodes: &[DeviceNode<'_>],
    output: &mut PciInfo,
) -> Result<()> {
    let node = walk.node();
    if node.raw_property("interrupt-map-pass-thru")?.is_some()
        || node.raw_property("msi-parent")?.is_some()
    {
        return Err("unsupported PCI interrupt routing binding".into());
    }
    if let Some(map) = property::<U32List<'_>>(node, "interrupt-map")? {
        // REFERENCE: https://www.devicetree.org/open-firmware/bindings/pci/pci2_1.pdf
        if property::<u32>(node, "#interrupt-cells")? != Some(1) {
            return Err("unsupported PCI interrupt cells".into());
        }
        let mask = property::<U32List<'_>>(node, "interrupt-map-mask")?
            .ok_or("PCI interrupt-map requires an address mask")?;
        let mut mask = mask.iter();
        let mask = {
            let values = [
                cell(&mut mask)?,
                cell(&mut mask)?,
                cell(&mut mask)?,
                cell(&mut mask)?,
            ];
            if mask.next().is_some() {
                return Err("invalid PCI interrupt mask size".into());
            }
            values
        };
        if mask[0] & !0x00ffff00 != 0 || mask[1] != 0 || mask[2] != 0 || mask[3] > 7 {
            return Err("unsupported PCI interrupt address mask".into());
        }
        let mut cells = map.iter().peekable();
        while cells.peek().is_some() {
            let address = [cell(&mut cells)?, cell(&mut cells)?, cell(&mut cells)?];
            let pin = cell(&mut cells)?;
            if !(1..=4).contains(&pin) {
                return Err("invalid PCI interrupt pin".into());
            }
            let phandle = cell(&mut cells)?;
            let (_, parent) = interrupts::controller(dtb, nodes, phandle)?;
            if property::<u32>(parent, "#address-cells")?.unwrap_or(0) != 0 {
                return Err("PCI interrupt parent address cells are unsupported".into());
            }
            let interrupt = interrupts::interrupt(dtb, nodes, phandle, &mut cells)?;
            let route = PciInterruptRoute {
                host: instance_id(index),
                requester_id: ((address[0] & mask[0]) >> 8) as u16,
                requester_mask: (mask[0] >> 8) as u16,
                pin: (pin & mask[3]) as u8,
                pin_mask: mask[3] as u8,
                interrupt,
            };
            if !(1..=4).contains(&route.pin) {
                return Err("invalid PCI interrupt route".into());
            }
            if output.interrupts.iter().any(|other| {
                route.host == other.host
                    && (route.requester_id ^ other.requester_id)
                        & route.requester_mask
                        & other.requester_mask
                        == 0
                    && (route.pin ^ other.pin) & route.pin_mask & other.pin_mask == 0
            }) {
                return Err("ambiguous PCI interrupt route".into());
            }
            output
                .interrupts
                .try_push(route)
                .map_err(|_| "too many PCI interrupt routes")?;
        }
    }
    msi_map(dtb, node, index, nodes, output)
}

fn msi_map(
    dtb: &Dtb<'_>,
    node: super::super::nodes::Node<'_>,
    index: usize,
    nodes: &[DeviceNode<'_>],
    output: &mut PciInfo,
) -> Result<()> {
    if let Some(map) = property::<U32List<'_>>(node, "msi-map")? {
        // REFERENCE: https://github.com/devicetree-org/dt-schema/blob/main/dtschema/schemas/pci/pci-msi.yaml
        let mask = property::<u32>(node, "msi-map-mask")?.unwrap_or(0xffff);
        if mask > 0xffff {
            return Err("PCI MSI requester mask exceeds 16 bits".into());
        }
        let mut cells = map.iter().peekable();
        while cells.peek().is_some() {
            let requester_base = cell(&mut cells)?;
            let controller = dtb.resolve_phandle(cell(&mut cells)?)?;
            let name = controller.name()?.name;
            let controller_index = nodes
                .iter()
                .position(|device| core::ptr::eq(device.name, name))
                .ok_or("PCI MSI controller is unsupported")?;
            if !interrupts::is_msi(controller)?
                || property::<u32>(controller, "#msi-cells")?.unwrap_or(0) != 0
            {
                return Err("unsupported PCI MSI controller binding".into());
            }
            let msi_base = cell(&mut cells)?;
            let requester_count = cell(&mut cells)?;
            if requester_count == 0
                || requester_base & !mask != 0
                || requester_base
                    .checked_add(requester_count)
                    .is_none_or(|end| end > 65536)
                || msi_base.checked_add(requester_count).is_none()
            {
                return Err("invalid PCI MSI requester map".into());
            }
            if output.msi_maps.iter().any(|other| {
                other.host == instance_id(index)
                    && (mask != u32::from(other.requester_mask)
                        || requester_base < other.requester_base + other.requester_count
                            && other.requester_base < requester_base + requester_count)
            }) {
                return Err("ambiguous PCI MSI requester map".into());
            }
            output
                .msi_maps
                .try_push(PciMsiMap {
                    host: instance_id(index),
                    requester_base,
                    requester_count,
                    requester_mask: mask as u16,
                    controller: instance_id(controller_index),
                    msi_base,
                })
                .map_err(|_| "too many PCI MSI requester maps")?;
        }
    }
    Ok(())
}

fn cell(cells: &mut impl Iterator<Item = u32>) -> Result<u32> {
    cells
        .next()
        .ok_or_else(|| "truncated PCI routing entry".into())
}

pub(super) fn validate(hardware: &HardwareInfo) -> Result<()> {
    for route in &hardware.pci.interrupts {
        let source = interrupts::source(hardware, route.interrupt)?;
        for interrupt in hardware
            .devices
            .iter()
            .flat_map(|device| &device.interrupts)
        {
            if interrupts::source(hardware, *interrupt)? == source {
                return Err("PCI interrupt overlaps a platform interrupt".into());
            }
        }
    }
    Ok(())
}
