// SPDX-License-Identifier: MPL-2.0

use alloc::vec::Vec;

use crate::{
    DeviceIdentity, DeviceProperty, DeviceUsePolicy, HardwareDeviceInfo, HardwareDeviceInstanceId,
    HardwareDeviceLocation, HardwareInfo, hardware,
    memory::{DeviceMapping, MemoryError, PhysRange},
};

use super::DeviceError;

const BUS_SIZE: u64 = 1 << 20;
const DEVICE_COUNT: u8 = 32;
const FUNCTION_COUNT: u8 = 8;

pub(super) fn enumerate(
    hardware: &HardwareInfo,
    first_id: u32,
    properties: &mut Vec<DeviceProperty>,
) -> Result<Vec<HardwareDeviceInfo>, DeviceError> {
    let mut enumerator = Enumerator {
        devices: Vec::new(),
        properties,
        next_id: first_id,
    };
    for device in &hardware.devices {
        if let Some(host) = PciHost::from_device(device) {
            enumerator.scan_host(host)?;
        }
    }
    Ok(enumerator.devices)
}

struct PciHost {
    id: HardwareDeviceInstanceId,
    segment: u16,
    first_bus: u8,
    last_bus: u8,
    ecam: PhysRange,
}

impl PciHost {
    fn from_device(device: &HardwareDeviceInfo) -> Option<Self> {
        let HardwareDeviceLocation::PciHostBridge {
            segment,
            first_bus,
            last_bus,
            ecam,
            ..
        } = &device.location
        else {
            return None;
        };
        Some(Self {
            id: device.instance_id,
            segment: *segment,
            first_bus: *first_bus,
            last_bus: *last_bus,
            ecam: *ecam,
        })
    }

    fn validate(&self) -> Result<(), DeviceError> {
        let count = u64::from(self.last_bus)
            .checked_sub(u64::from(self.first_bus))
            .and_then(|count| count.checked_add(1))
            .ok_or(DeviceError::InvalidArgument)?;
        let end = self
            .ecam
            .start
            .checked_add(count * BUS_SIZE)
            .ok_or(DeviceError::InvalidArgument)?;
        if end > self.ecam.end {
            return Err(DeviceError::InvalidArgument);
        }
        Ok(())
    }
}

struct Enumerator<'a> {
    devices: Vec<HardwareDeviceInfo>,
    properties: &'a mut Vec<DeviceProperty>,
    next_id: u32,
}

impl Enumerator<'_> {
    fn scan_host(&mut self, host: PciHost) -> Result<(), DeviceError> {
        host.validate()?;
        let mut buses = Vec::new();
        buses
            .try_reserve_exact(256)
            .map_err(|_| DeviceError::Memory(MemoryError::NoMemory))?;
        let mut used_buses = [false; 256];
        used_buses[usize::from(host.first_bus)] = true;
        let mut mapping = bus_mapping(&host, host.first_bus)?;
        let reserved = bridge_bus_ranges(&mapping, host.first_bus, host.last_bus)?;
        buses.push(BusFrame {
            bus: host.first_bus,
            bus_limit: host.last_bus,
            max_bus: host.first_bus,
            slot: 0,
            function: 0,
            upstream: None,
            reserved,
        });
        while !buses.is_empty() {
            let bus = buses.last().unwrap().bus;
            let Some((slot, function, info)) = buses.last_mut().unwrap().next(&mapping)? else {
                let Some(parent_mapping) = finish_bus(&host, &mut buses, mapping)? else {
                    break;
                };
                mapping = parent_mapping;
                continue;
            };
            let child = self.visit_function(
                &host,
                &mut buses,
                &mapping,
                &mut used_buses,
                FunctionAddress {
                    bus,
                    slot,
                    function,
                },
                info,
            )?;
            if let Some(child) = child {
                mapping = enter_bus(&host, &mut buses, child, mapping)?;
            }
        }
        Ok(())
    }

    fn visit_function(
        &mut self,
        host: &PciHost,
        buses: &mut [BusFrame],
        mapping: &DeviceMapping,
        used_buses: &mut [bool; 256],
        address: FunctionAddress,
        info: FunctionInfo,
    ) -> Result<Option<BusFrame>, DeviceError> {
        if info.upstream_bridge(address.bus) {
            return Ok(None);
        }
        let parent_frame = buses.last().unwrap();
        let parent = parent_frame.upstream.map_or(host.id, |upstream| {
            HardwareDeviceInstanceId(upstream.instance_id)
        });
        let instance_id = self.record(
            host,
            parent,
            address.bus,
            address.slot,
            address.function,
            info,
        )?;
        if info.header_type != 1 {
            return Ok(None);
        }

        let bus_limit = parent_frame.bus_limit;
        let mut reserved = parent_frame.reserved;
        let (secondary, subordinate, dynamic) = if info.unconfigured_bridge(address.bus) {
            let secondary = allocate_bus(address.bus, bus_limit, used_buses, &reserved)?;
            let register = config(mapping, address.slot, address.function, 0x18)?;
            let register = (register & 0xff00_0000)
                | u32::from(address.bus)
                | (u32::from(secondary) << 8)
                | (u32::from(bus_limit) << 16);
            write_config(mapping, address.slot, address.function, 0x18, register)?;
            (secondary, bus_limit, true)
        } else {
            if info.primary_bus != address.bus
                || info.secondary_bus <= address.bus
                || info.secondary_bus > info.subordinate_bus
                || info.subordinate_bus > bus_limit
            {
                return Err(DeviceError::InvalidArgument);
            }
            (info.secondary_bus, info.subordinate_bus, false)
        };
        if used_buses[usize::from(secondary)] {
            return Err(DeviceError::InvalidArgument);
        }
        used_buses[usize::from(secondary)] = true;
        let frame = buses.last_mut().unwrap();
        frame.max_bus = frame
            .max_bus
            .max(if dynamic { secondary } else { subordinate });
        if !dynamic {
            reserved[usize::from(secondary)..=usize::from(subordinate)].fill(false);
        }
        Ok(Some(BusFrame {
            bus: secondary,
            bus_limit: subordinate,
            max_bus: secondary,
            slot: 0,
            function: 0,
            upstream: Some(Upstream {
                slot: address.slot,
                function: address.function,
                instance_id,
                secondary,
                dynamic,
            }),
            reserved,
        }))
    }

    fn record(
        &mut self,
        host: &PciHost,
        parent: HardwareDeviceInstanceId,
        bus: u8,
        slot: u8,
        function: u8,
        info: FunctionInfo,
    ) -> Result<u32, DeviceError> {
        record(
            &mut self.devices,
            self.properties,
            &mut self.next_id,
            parent,
            Bdf {
                segment: host.segment,
                bus,
                slot,
                function,
            },
            info,
        )
    }
}

fn finish_bus(
    host: &PciHost,
    buses: &mut Vec<BusFrame>,
    mapping: DeviceMapping,
) -> Result<Option<DeviceMapping>, DeviceError> {
    let completed = buses.pop().unwrap();
    let Some(upstream) = completed.upstream else {
        return Ok(None);
    };
    let parent_bus = buses.last().unwrap().bus;
    drop(mapping);
    let mapping = bus_mapping(host, parent_bus)?;
    let parent = buses.last_mut().unwrap();
    parent.max_bus = parent.max_bus.max(completed.max_bus);
    if upstream.dynamic {
        parent.reserved[usize::from(upstream.secondary)..=usize::from(completed.max_bus)]
            .fill(true);
        write_subordinate(
            &mapping,
            upstream.slot,
            upstream.function,
            completed.max_bus,
        )?;
    }
    Ok(Some(mapping))
}

fn enter_bus(
    host: &PciHost,
    buses: &mut Vec<BusFrame>,
    mut child: BusFrame,
    mapping: DeviceMapping,
) -> Result<DeviceMapping, DeviceError> {
    drop(mapping);
    let mapping = bus_mapping(host, child.bus)?;
    let child_ranges = bridge_bus_ranges(&mapping, child.bus, child.bus_limit)?;
    if child_ranges
        .iter()
        .zip(child.reserved.iter())
        .any(|(child, inherited)| *child && *inherited)
    {
        return Err(DeviceError::InvalidArgument);
    }
    for (reserved, child) in child.reserved.iter_mut().zip(child_ranges) {
        *reserved |= child;
    }
    buses.push(child);
    Ok(mapping)
}

struct BusFrame {
    bus: u8,
    bus_limit: u8,
    max_bus: u8,
    slot: u8,
    function: u8,
    upstream: Option<Upstream>,
    reserved: [bool; 256],
}

#[derive(Clone, Copy)]
struct Upstream {
    slot: u8,
    function: u8,
    instance_id: u32,
    secondary: u8,
    dynamic: bool,
}

impl BusFrame {
    fn next(
        &mut self,
        mapping: &DeviceMapping,
    ) -> Result<Option<(u8, u8, FunctionInfo)>, DeviceError> {
        while self.slot < DEVICE_COUNT {
            if self.function == 0 {
                let slot = self.slot;
                let Some(info) = read_function(mapping, slot, 0)? else {
                    self.slot += 1;
                    continue;
                };
                if info.multifunction {
                    self.function = 1;
                } else {
                    self.slot += 1;
                }
                return Ok(Some((slot, 0, info)));
            }
            let slot = self.slot;
            let function = self.function;
            self.function += 1;
            if self.function == FUNCTION_COUNT {
                self.function = 0;
                self.slot += 1;
            }
            if let Some(info) = read_function(mapping, slot, function)? {
                return Ok(Some((slot, function, info)));
            }
        }
        Ok(None)
    }
}

#[derive(Clone, Copy)]
struct FunctionInfo {
    vendor: u16,
    device: u16,
    programming_interface: u8,
    subclass: u8,
    class: u8,
    header_type: u8,
    multifunction: bool,
    primary_bus: u8,
    secondary_bus: u8,
    subordinate_bus: u8,
}

#[derive(Clone, Copy)]
struct FunctionAddress {
    bus: u8,
    slot: u8,
    function: u8,
}

impl FunctionInfo {
    fn unconfigured_bridge(self, bus: u8) -> bool {
        self.secondary_bus == 0
            && self.subordinate_bus == 0
            && (self.primary_bus == 0 || self.primary_bus == bus)
    }

    fn upstream_bridge(self, bus: u8) -> bool {
        self.header_type == 1
            && self.primary_bus < bus
            && self.secondary_bus <= bus
            && bus <= self.subordinate_bus
    }
}

fn read_function(
    mapping: &DeviceMapping,
    slot: u8,
    function: u8,
) -> Result<Option<FunctionInfo>, DeviceError> {
    let id = config(mapping, slot, function, 0)?;
    let vendor = id as u16;
    if vendor == u16::MAX {
        return Ok(None);
    }
    let class = config(mapping, slot, function, 0x08)?;
    let header = config(mapping, slot, function, 0x0c)?;
    let header_type = (header >> 16) as u8;
    let (primary_bus, secondary_bus, subordinate_bus) = if header_type & 0x7f == 1 {
        let buses = config(mapping, slot, function, 0x18)?;
        (buses as u8, (buses >> 8) as u8, (buses >> 16) as u8)
    } else {
        (0, 0, 0)
    };
    Ok(Some(FunctionInfo {
        vendor,
        device: (id >> 16) as u16,
        programming_interface: (class >> 8) as u8,
        subclass: (class >> 16) as u8,
        class: (class >> 24) as u8,
        header_type: header_type & 0x7f,
        multifunction: header_type & 0x80 != 0,
        primary_bus,
        secondary_bus,
        subordinate_bus,
    }))
}

fn bridge_bus_ranges(
    mapping: &DeviceMapping,
    bus: u8,
    bus_limit: u8,
) -> Result<[bool; 256], DeviceError> {
    let mut reserved = [false; 256];
    for slot in 0..DEVICE_COUNT {
        let Some(first) = read_function(mapping, slot, 0)? else {
            continue;
        };
        reserve_bridge_range(&mut reserved, bus, bus_limit, first)?;
        if first.multifunction {
            for function in 1..FUNCTION_COUNT {
                if let Some(info) = read_function(mapping, slot, function)? {
                    reserve_bridge_range(&mut reserved, bus, bus_limit, info)?;
                }
            }
        }
    }
    Ok(reserved)
}

fn reserve_bridge_range(
    reserved: &mut [bool; 256],
    bus: u8,
    bus_limit: u8,
    info: FunctionInfo,
) -> Result<(), DeviceError> {
    if info.header_type != 1 || info.unconfigured_bridge(bus) || info.upstream_bridge(bus) {
        return Ok(());
    }
    if info.primary_bus != bus
        || info.secondary_bus <= bus
        || info.secondary_bus > info.subordinate_bus
        || info.subordinate_bus > bus_limit
    {
        return Err(DeviceError::InvalidArgument);
    }
    let range = usize::from(info.secondary_bus)..=usize::from(info.subordinate_bus);
    if range.clone().any(|number| reserved[number]) {
        return Err(DeviceError::InvalidArgument);
    }
    reserved[range].fill(true);
    Ok(())
}

fn allocate_bus(
    bus: u8,
    bus_limit: u8,
    used: &[bool; 256],
    reserved: &[bool; 256],
) -> Result<u8, DeviceError> {
    (u16::from(bus) + 1..=u16::from(bus_limit))
        .map(|number| number as usize)
        .find(|number| !used[*number] && !reserved[*number])
        .map(|number| number as u8)
        .ok_or(DeviceError::InvalidArgument)
}

fn config(
    mapping: &DeviceMapping,
    slot: u8,
    function: u8,
    register: usize,
) -> Result<u32, DeviceError> {
    let offset = (usize::from(slot) * usize::from(FUNCTION_COUNT) + usize::from(function))
        .checked_mul(4096)
        .and_then(|base| base.checked_add(register))
        .ok_or(DeviceError::InvalidArgument)?;
    hardware::pci_config()
        .read32(mapping, offset)
        .map_err(DeviceError::Pci)
}

fn write_config(
    mapping: &DeviceMapping,
    slot: u8,
    function: u8,
    register: usize,
    value: u32,
) -> Result<(), DeviceError> {
    let offset = (usize::from(slot) * usize::from(FUNCTION_COUNT) + usize::from(function))
        .checked_mul(4096)
        .and_then(|base| base.checked_add(register))
        .ok_or(DeviceError::InvalidArgument)?;
    hardware::pci_config()
        .write32(mapping, offset, value)
        .map_err(DeviceError::Pci)
}

fn write_subordinate(
    mapping: &DeviceMapping,
    slot: u8,
    function: u8,
    subordinate: u8,
) -> Result<(), DeviceError> {
    let buses = config(mapping, slot, function, 0x18)?;
    let buses = (buses & !(0xff << 16)) | (u32::from(subordinate) << 16);
    write_config(mapping, slot, function, 0x18, buses)
}

fn bus_mapping(host: &PciHost, bus: u8) -> Result<DeviceMapping, DeviceError> {
    let offset = u64::from(bus - host.first_bus) * BUS_SIZE;
    let start = host
        .ecam
        .start
        .checked_add(offset)
        .ok_or(DeviceError::InvalidArgument)?;
    DeviceMapping::new(PhysRange {
        start,
        end: start
            .checked_add(BUS_SIZE)
            .ok_or(DeviceError::InvalidArgument)?,
    })
    .map_err(DeviceError::Memory)
}

#[derive(Clone, Copy)]
struct Bdf {
    segment: u16,
    bus: u8,
    slot: u8,
    function: u8,
}

fn record(
    output: &mut Vec<HardwareDeviceInfo>,
    properties: &mut Vec<DeviceProperty>,
    next_id: &mut u32,
    parent: HardwareDeviceInstanceId,
    bdf: Bdf,
    info: FunctionInfo,
) -> Result<u32, DeviceError> {
    let mut name = *b"PCI 0000:00:00.0";
    let mut bdf_text = *b"0000:00:00.0";
    let mut vendor_device = *b"0000:0000";
    let mut class = *b"00:00:00";
    encode_hex(u32::from(bdf.segment), &mut bdf_text[..4]);
    encode_hex(u32::from(bdf.bus), &mut bdf_text[5..7]);
    encode_hex(u32::from(bdf.slot), &mut bdf_text[8..10]);
    bdf_text[11] = hex_digit(bdf.function);
    name[4..].copy_from_slice(&bdf_text);
    encode_hex(u32::from(info.vendor), &mut vendor_device[..4]);
    encode_hex(u32::from(info.device), &mut vendor_device[5..]);
    encode_hex(u32::from(info.class), &mut class[..2]);
    encode_hex(u32::from(info.subclass), &mut class[3..5]);
    encode_hex(u32::from(info.programming_interface), &mut class[6..]);
    output
        .try_reserve(1)
        .map_err(|_| DeviceError::Memory(MemoryError::NoMemory))?;
    properties
        .try_reserve(1)
        .map_err(|_| DeviceError::Memory(MemoryError::NoMemory))?;
    let instance_id = *next_id;
    let next_instance_id = instance_id
        .checked_add(1)
        .ok_or(DeviceError::Memory(MemoryError::NoMemory))?;
    let mut device = HardwareDeviceInfo::new(HardwareDeviceInstanceId(instance_id));
    device.name = core::str::from_utf8(&name)
        .map_err(|_| DeviceError::InvalidArgument)?
        .try_into()
        .map_err(|_| DeviceError::InvalidArgument)?;
    device.parent = Some(parent);
    device.use_policy = DeviceUsePolicy::KernelReserved;
    device.location = HardwareDeviceLocation::Pci {
        segment: bdf.segment,
        bus: bdf.bus,
        slot: bdf.slot,
        function: bdf.function,
    };
    device
        .identities
        .try_push(
            DeviceIdentity::new(
                "pci",
                core::str::from_utf8(&vendor_device).map_err(|_| DeviceError::InvalidArgument)?,
            )
            .map_err(|_| DeviceError::InvalidArgument)?,
        )
        .map_err(|_| DeviceError::InvalidArgument)?;
    device
        .identities
        .try_push(
            DeviceIdentity::new(
                "pci-class",
                core::str::from_utf8(&class).map_err(|_| DeviceError::InvalidArgument)?,
            )
            .map_err(|_| DeviceError::InvalidArgument)?,
        )
        .map_err(|_| DeviceError::InvalidArgument)?;
    properties.push(
        DeviceProperty::new(
            HardwareDeviceInstanceId(instance_id),
            "pci",
            "bdf",
            &bdf_text,
        )
        .map_err(|_| DeviceError::InvalidArgument)?,
    );
    output.push(device);
    *next_id = next_instance_id;
    Ok(instance_id)
}

fn encode_hex(mut value: u32, output: &mut [u8]) {
    for byte in output.iter_mut().rev() {
        *byte = hex_digit(value as u8);
        value >>= 4;
    }
}

fn hex_digit(value: u8) -> u8 {
    b"0123456789abcdef"[usize::from(value & 0x0f)]
}
