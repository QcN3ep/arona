// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Discover devices and normalize address, interrupt, and DMA resources while preserving device-specific properties.

pub mod flash;
mod interrupts;
mod pci;
pub mod serial;
pub mod syscon;

use core::{fmt::Write, ops::Range};

use arrayvec::ArrayVec;
use fdt::properties::{Compatible, values::PropertyValue};
use kernel::{
    DeviceIdentity, DeviceProperty, DeviceUsePolicy, DmaCapability, DmaInfo, HardwareDeviceInfo,
    HardwareDeviceInstanceId, HardwareDeviceLocation, HardwareInfo, PciWindow,
    boot::{MAX_DEVICE_PROPERTIES, MAX_DEVICES, MAX_MMIO_REGIONS},
    memory::PhysRange,
};

use super::{
    Dtb, Result,
    nodes::{Node, Nodes, cell_sizes, property, registers},
    range,
};

const PAGE_SIZE: u64 = 4096;

#[derive(Debug)]
struct DeviceNode<'a> {
    // fdt's NodeId assumes the strings block follows the structure block; identify a node here by its node name's position in the DTB.
    name: &'a str,
}

pub(super) fn parse(dtb: &Dtb<'_>, hardware: &mut HardwareInfo) -> Result<()> {
    let mut nodes = const { ArrayVec::new_const() };
    collect_nodes(dtb, &mut nodes)?;
    parse_devices(dtb, hardware, &nodes)?;
    syscon::capture(hardware)?;
    hardware.console = serial::console(dtb, &nodes).unwrap_or_default();
    for (index, device) in hardware.devices.iter().enumerate() {
        validate_addresses(hardware, index, device)?;
        if hardware.devices[..index]
            .iter()
            .any(|other| locations_overlap(&device.location, &other.location))
        {
            return Err("device locations overlap".into());
        }
    }
    interrupts::validate(hardware)?;
    pci::validate(hardware)
}

fn parse_devices(
    dtb: &Dtb<'_>,
    hardware: &mut HardwareInfo,
    nodes: &[DeviceNode<'_>],
) -> Result<()> {
    hardware.devices.clear();
    hardware.device_properties.clear();
    hardware.cpu_interrupts.clear();
    hardware.pci.interrupts.clear();
    hardware.pci.msi_maps.clear();
    let mut walk = dtb.nodes();
    while let Some(node) = walk.next()? {
        let Some(index) = node_index(nodes, node)? else {
            continue;
        };
        let position = hardware
            .devices
            .partition_point(|device| device.instance_id.0 < instance_id(index).0);
        hardware
            .devices
            .insert(position, HardwareDeviceInfo::new(instance_id(index)));
        let device = &mut hardware.devices[position];
        parse_device(&walk, nodes, device)?;
        interrupts::resources(dtb, &walk, nodes, device, &mut hardware.cpu_interrupts)?;
        capture_properties(node, device, &mut hardware.device_properties)?;
        if matches!(
            device.location,
            HardwareDeviceLocation::PciHostBridge { .. }
        ) {
            pci::routes(dtb, &walk, index, nodes, &mut hardware.pci)?;
        }
    }
    Ok(())
}

fn collect_nodes<'a>(
    dtb: &Dtb<'a>,
    output: &mut ArrayVec<DeviceNode<'a>, MAX_DEVICES>,
) -> Result<()> {
    let mut entries = ArrayVec::<(Range<usize>, DeviceNode<'a>), MAX_DEVICES>::new();
    let mut paths = ArrayVec::<u8, 8192>::new();
    let mut walk = dtb.nodes();
    while let Some(node) = walk.next()? {
        if !is_device(&walk)? {
            continue;
        }
        let start = paths.len();
        for byte in walk.path() {
            paths
                .try_push(byte)
                .map_err(|_| "device paths exceed 8 KiB")?;
        }
        entries
            .try_push((
                start..paths.len(),
                DeviceNode {
                    name: node.name()?.name,
                },
            ))
            .map_err(|_| "too many devices")?;
    }
    entries
        .as_mut_slice()
        .sort_unstable_by(|(a, _), (b, _)| paths[a.clone()].cmp(&paths[b.clone()]));
    if entries
        .windows(2)
        .any(|pair| paths[pair[0].0.clone()] == paths[pair[1].0.clone()])
    {
        return Err("duplicate device path".into());
    }
    output.extend(entries.drain(..).map(|(_, node)| node));
    Ok(())
}

fn is_device(walk: &Nodes<'_>) -> Result<bool> {
    if walk.node().parent().is_none() {
        return Ok(false);
    }
    for node in core::iter::once(walk.node()).chain(walk.ancestors()) {
        if property::<&str>(node, "device_type")? == Some("memory")
            || node
                .parent()
                .is_some_and(|parent| parent.parent().is_none())
                && matches!(
                    node.name()?.name,
                    "cpus" | "reserved-memory" | "chosen" | "aliases"
                )
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn parse_device(
    walk: &Nodes<'_>,
    nodes: &[DeviceNode<'_>],
    device: &mut HardwareDeviceInfo,
) -> Result<()> {
    let node = walk.node();
    write!(device.name, "{}", node.name()?).map_err(|_| "device name is too long")?;
    for ancestor in walk.ancestors() {
        if let Some(index) = node_index(nodes, ancestor)? {
            device.parent = Some(instance_id(index));
            break;
        }
    }
    if let Some(compatible) = node.property::<Compatible>()? {
        for name in compatible {
            device
                .identities
                .try_push(DeviceIdentity::new("devicetree", name)?)
                .map_err(|_| "too many device identities")?;
        }
    }
    device.location = if matches(node, "pci-host-ecam-generic")? {
        pci::parse(walk)?
    } else {
        interrupts::location(walk)?.unwrap_or(HardwareDeviceLocation::Platform)
    };
    if device.location == HardwareDeviceLocation::Platform {
        device.mmio_regions = mmio(walk)?;
    }
    device.dma = dma(walk)?;
    if node.raw_property("interrupt-controller")?.is_some()
        || node.raw_property("msi-controller")?.is_some()
        || node.property::<Compatible>()?.is_some_and(|names| {
            names.into_iter().any(|name| {
                matches!(
                    name,
                    "riscv,clint0"
                        | "sifive,clint0"
                        | "syscon"
                        | "syscon-poweroff"
                        | "syscon-reboot"
                )
            })
        })
    {
        device.use_policy = DeviceUsePolicy::KernelReserved;
    }
    Ok(())
}

fn capture_properties(
    node: Node<'_>,
    device: &HardwareDeviceInfo,
    properties: &mut ArrayVec<DeviceProperty, MAX_DEVICE_PROPERTIES>,
) -> Result<()> {
    let pci = matches!(
        device.location,
        HardwareDeviceLocation::PciHostBridge { .. }
    );
    for entry in node.properties()? {
        let entry = entry?;
        if matches!(
            entry.name,
            "compatible" | "status" | "interrupts" | "interrupts-extended"
        ) || entry.name == "reg"
            && (!device.mmio_regions.is_empty()
                || device.location != HardwareDeviceLocation::Platform)
            || pci
                && matches!(
                    entry.name,
                    "ranges"
                        | "interrupt-map"
                        | "interrupt-map-mask"
                        | "msi-map"
                        | "msi-map-mask"
                        | "bus-range"
                        | "linux,pci-domain"
                )
        {
            continue;
        }
        if properties
            .iter()
            .any(|other| other.device == device.instance_id && other.name.as_str() == entry.name)
        {
            return Err("duplicate device property".into());
        }
        properties
            .try_push(DeviceProperty::new(
                device.instance_id,
                "devicetree",
                entry.name,
                entry.value,
            )?)
            .map_err(|_| "too many device properties")?;
    }
    Ok(())
}

fn instance_id(index: usize) -> HardwareDeviceInstanceId {
    HardwareDeviceInstanceId((index + 1) as u32)
}

fn node_index(nodes: &[DeviceNode<'_>], node: Node<'_>) -> Result<Option<usize>> {
    let name = node.name()?.name;
    Ok(nodes
        .iter()
        .position(|device| core::ptr::eq(device.name, name)))
}

fn find_device(
    hardware: &HardwareInfo,
    id: HardwareDeviceInstanceId,
) -> Result<&HardwareDeviceInfo> {
    hardware
        .devices
        .iter()
        .find(|device| device.instance_id == id)
        .ok_or_else(|| "referenced device is missing from the device table".into())
}

fn mmio(walk: &Nodes<'_>) -> Result<ArrayVec<PhysRange, MAX_MMIO_REGIONS>> {
    let mut output = ArrayVec::new();
    if walk.node().raw_property("reg")?.is_none() {
        return Ok(output);
    }
    for bus in walk.ancestors() {
        let cells = cell_sizes(bus)?;
        if cells.size_cells == 0
            || cells.address_cells > 2
            || bus.parent().is_some() && bus.raw_property("ranges")?.is_none()
        {
            return Ok(output);
        }
    }
    for entry in registers(walk.node())?.iter::<u64, u64>() {
        let entry = entry?;
        let address = walk.translate(range(entry.address, entry.len)?)?;
        output
            .try_push(address)
            .map_err(|_| "too many MMIO ranges")?;
    }
    Ok(output)
}

fn matches(node: Node<'_>, identity: &str) -> Result<bool> {
    Ok(node
        .property::<Compatible>()?
        .is_some_and(|names| names.into_iter().any(|name| name == identity)))
}

fn dma(walk: &Nodes<'_>) -> Result<DmaCapability> {
    let node = walk.node();
    if !(matches(node, "virtio,mmio")?
        || matches(node, "pci-host-ecam-generic")?
        || matches(node, "qemu,fw-cfg-mmio")?)
    {
        let capability = if matches(node, "ns16550a")?
            || matches(node, "ns16550")?
            || matches(node, "cfi-flash")?
            || matches(node, "google,goldfish-rtc")?
            || matches(node, "loongson,ls7a-rtc")?
        {
            DmaCapability::None
        } else {
            DmaCapability::Unknown
        };
        return Ok(capability);
    }
    if node.raw_property("iommus")?.is_some() || node.raw_property("iommu-map")?.is_some() {
        return Err("IOMMU device bindings are not supported".into());
    }
    let mut coherent = None;
    for bus in core::iter::once(node).chain(walk.ancestors()) {
        if bus
            .raw_property("dma-ranges")?
            .is_some_and(|property| !property.value.is_empty())
        {
            return Err("DMA address translation is not supported".into());
        }
        let yes = bus.raw_property("dma-coherent")?.is_some();
        let no = bus.raw_property("dma-noncoherent")?.is_some();
        if yes && no {
            return Err("conflicting DMA coherency properties".into());
        }
        if coherent.is_none() && (yes || no) {
            coherent = Some(yes);
        }
    }
    Ok(DmaCapability::Present(DmaInfo {
        address_bits: 64,
        coherent: coherent.unwrap_or(false),
        isolation: None,
    }))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegisterEndian {
    Little,
    Big,
}

fn attribute<'a, T: PropertyValue<'a>>(
    hardware: &'a HardwareInfo,
    device: &HardwareDeviceInfo,
    name: &str,
) -> Result<Option<T>> {
    hardware
        .device_property(device.instance_id, "devicetree", name)
        .map(|value| T::parse(value).map_err(|error| fdt::FdtError::from(error).into()))
        .transpose()
}

fn resolve(hardware: &HardwareInfo, phandle: u32) -> Result<&HardwareDeviceInfo> {
    if phandle == 0 || phandle == u32::MAX {
        return Err("invalid phandle".into());
    }
    let mut found = None;
    for device in &hardware.devices {
        if attribute::<u32>(hardware, device, "phandle")? == Some(phandle)
            && found.replace(device).is_some()
        {
            return Err("duplicate phandle".into());
        }
    }
    found.ok_or_else(|| "phandle does not reference a device".into())
}

fn endian(hardware: &HardwareInfo, device: &HardwareDeviceInfo) -> Result<RegisterEndian> {
    let big = hardware
        .device_property(device.instance_id, "devicetree", "big-endian")
        .is_some();
    let little = hardware
        .device_property(device.instance_id, "devicetree", "little-endian")
        .is_some();
    let native = hardware
        .device_property(device.instance_id, "devicetree", "native-endian")
        .is_some();
    if u8::from(big) + u8::from(little) + u8::from(native) > 1 {
        return Err("conflicting register endianness".into());
    }
    Ok(if big || native && cfg!(target_endian = "big") {
        RegisterEndian::Big
    } else {
        RegisterEndian::Little
    })
}

fn validate_addresses(
    hardware: &HardwareInfo,
    index: usize,
    device: &HardwareDeviceInfo,
) -> Result<()> {
    for (position, address) in addresses(device).enumerate() {
        let pages = address.page_envelope()?;
        if hardware
            .memory
            .regions
            .iter()
            .any(|region| region.range.overlaps(pages))
        {
            return Err("device resource envelope overlaps RAM".into());
        }
        if addresses(device)
            .take(position)
            .any(|other| address.overlaps(other))
        {
            return Err("device resources overlap within the same device".into());
        }
        for other in &hardware.devices[..index] {
            for other_address in addresses(other) {
                if address.overlaps(other_address) {
                    return Err("device register ranges overlap".into());
                }
            }
        }
    }
    Ok(())
}

fn addresses(device: &HardwareDeviceInfo) -> impl Iterator<Item = PhysRange> + '_ {
    let (ecam, windows): (_, &[PciWindow]) = match &device.location {
        HardwareDeviceLocation::PciHostBridge { ecam, windows, .. } => {
            (Some(*ecam), windows.as_slice())
        }
        HardwareDeviceLocation::Platform
        | HardwareDeviceLocation::Iocsr { .. }
        | HardwareDeviceLocation::Pci { .. } => (None, &[]),
    };
    device
        .mmio_regions
        .iter()
        .copied()
        .chain(ecam)
        .chain(windows.iter().map(|window| window.cpu_range))
}

fn locations_overlap(left: &HardwareDeviceLocation, right: &HardwareDeviceLocation) -> bool {
    match (left, right) {
        (
            HardwareDeviceLocation::Iocsr { registers: a },
            HardwareDeviceLocation::Iocsr { registers: b },
        ) => a.start < b.end && b.start < a.end,
        (
            HardwareDeviceLocation::PciHostBridge {
                segment: a_segment,
                first_bus: a_first,
                last_bus: a_last,
                ..
            },
            HardwareDeviceLocation::PciHostBridge {
                segment: b_segment,
                first_bus: b_first,
                last_bus: b_last,
                ..
            },
        ) => a_segment == b_segment && a_last >= b_first && b_last >= a_first,
        _ => false,
    }
}
