// SPDX-License-Identifier: MPL-2.0

//! 解析 generic ECAM 主桥的总线范围和内存窗口。

use arrayvec::ArrayVec;
use fdt::properties::{
    pci::{PciAddress, PciAddressSpace},
    values::U32List,
};
use kernel::HardwareDeviceLocation;

use super::super::{
    Result,
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
        if !matches!(
            entry.child_bus_address.hi.address_space(),
            PciAddressSpace::Memory32 | PciAddressSpace::Memory64
        ) {
            continue;
        }
        let child =
            u64::from(entry.child_bus_address.mid) << 32 | u64::from(entry.child_bus_address.lo);
        let window = walk.translate(range(entry.parent_bus_address, entry.len)?)?;
        if child != window.start {
            return Err("non-identity PCI memory windows are not supported".into());
        }
        if !window.start.is_multiple_of(PAGE_SIZE) || !window.end.is_multiple_of(PAGE_SIZE) {
            return Err("PCI memory window is not page-aligned".into());
        }
        windows
            .try_push(window)
            .map_err(|_| "too many PCI memory windows")?;
    }
    Ok(HardwareDeviceLocation::PciHostBridge {
        segment,
        first_bus: first as u8,
        last_bus: last as u8,
        ecam,
        memory_windows: windows,
        iommu: None,
    })
}
