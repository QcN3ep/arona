// SPDX-License-Identifier: MPL-2.0

use alloc::{format, string::String, vec::Vec};
use kernel::{
    HardwareDeviceLocation, HardwareInfo, PciAddressSpace,
    device::{DeviceError, DeviceField, DeviceSet},
    hardware::{
        self,
        mmio::{DeviceMapper, MmioMapper},
    },
    memory::{DeviceMapping, PhysRange},
};

pub struct Expected {
    bus: u8,
    slot: usize,
    name: String,
    offset: usize,
    value: u32,
}

pub fn prepare(suite: &str, hardware: &mut HardwareInfo) -> Vec<Expected> {
    if suite == "pci-exhausted" {
        constrain(hardware);
    }
    let expected = if matches!(suite, "pci-firmware" | "pci-rollback") {
        firmware(hardware)
    } else {
        Vec::new()
    };
    if suite == "pci-rollback" {
        rollback(hardware, &expected);
    }
    expected
}

pub fn firmware(hardware: &mut HardwareInfo) -> Vec<Expected> {
    let host = hardware
        .devices
        .iter()
        .position(|host| matches!(host.location, HardwareDeviceLocation::PciHostBridge { .. }))
        .unwrap();
    let (segment, first_bus, memory_bus, prefetch_bus) = {
        let HardwareDeviceLocation::PciHostBridge {
            segment,
            first_bus,
            windows,
            ..
        } = &hardware.devices[host].location
        else {
            unreachable!()
        };
        let memory = windows
            .iter()
            .find(|window| window.space == PciAddressSpace::Memory32 && !window.prefetchable)
            .unwrap();
        let prefetch = windows
            .iter()
            .find(|window| window.space == PciAddressSpace::Memory64)
            .unwrap_or(memory);
        (
            *segment,
            *first_bus,
            memory.bus_address,
            prefetch.bus_address,
        )
    };
    let base = memory_bus + 0x10_0000;
    let wide = prefetch_bus + 0x400_0000;
    let mapping = map_bus(hardware, host, first_bus);
    let access = hardware::pci_config();
    let mut expected = Vec::new();
    let mut write = |slot: usize, offset: usize, value: u32| {
        access
            .write32(&mapping, slot * 8 * 4096 + offset, value)
            .unwrap();
        expected.push(Expected {
            bus: first_bus,
            slot,
            name: format!("PCI {segment:04x}:{first_bus:02x}:{slot:02x}.0"),
            offset,
            value,
        });
    };
    let testdev = access.read32(&mapping, 8 * 4096).unwrap() == 0x0005_1b36;
    let (memory_bar, wide_bar) = if testdev { (0x10, 0x18) } else { (0x14, 0x20) };
    write(1, memory_bar, base as u32);
    write(1, wide_bar, wide as u32 | 0xc);
    write(1, wide_bar + 4, (wide >> 32) as u32);
    let secondary = first_bus + 5;
    write(
        2,
        0x18,
        u32::from(first_bus) | u32::from(secondary) << 8 | u32::from(secondary) << 16,
    );
    let bridge_base = base + 0x20_0000;
    let bridge_end = bridge_base + 0x20_0000;
    write(
        2,
        0x20,
        (bridge_base as u32 >> 16) | ((bridge_end - 1) as u32 & 0xfff0_0000),
    );
    let bridge_wide = wide + 0x100_0000;
    let bridge_prefetch = ((bridge_wide >> 16) as u32 & 0xfff0)
        | ((bridge_wide + 0x10_0000 - 1) as u32 & 0xfff0_0000)
        | (access.read32(&mapping, 2 * 8 * 4096 + 0x24).unwrap() & 0x000f_000f);
    write(2, 0x24, bridge_prefetch);
    write(2, 0x28, (bridge_wide >> 32) as u32);
    write(2, 0x2c, ((bridge_wide + 0x10_0000 - 1) >> 32) as u32);
    write(7, 0x10, (base + 0x60_0000) as u32);
    for slot in [1, 2, 7] {
        access.write16(&mapping, slot * 8 * 4096 + 4, 7).unwrap();
    }
    drop(mapping);
    let mapping = map_bus(hardware, host, secondary);
    access
        .write32(&mapping, 8 * 4096 + 0x10, (bridge_base + 0x2_0000) as u32)
        .unwrap();
    access.write16(&mapping, 8 * 4096 + 4, 7).unwrap();
    expected.push(Expected {
        bus: secondary,
        slot: 1,
        name: format!("PCI {segment:04x}:{secondary:02x}:01.0"),
        offset: 0x10,
        value: (bridge_base + 0x2_0000) as u32,
    });
    for (offset, value) in [
        (0x14, (bridge_base + 0x10_0000) as u32),
        (0x20, (bridge_wide + 0x1_0000) as u32 | 0xc),
        (0x24, ((bridge_wide + 0x1_0000) >> 32) as u32),
    ] {
        access
            .write32(&mapping, 2 * 8 * 4096 + offset, value)
            .unwrap();
        expected.push(Expected {
            bus: secondary,
            slot: 2,
            name: format!("PCI {segment:04x}:{secondary:02x}:02.0"),
            offset,
            value,
        });
    }
    expected
}

pub fn verify(devices: &DeviceSet, expected: &[Expected]) {
    for entry in expected {
        let mut cursor = 0;
        let id = loop {
            let id = devices
                .next(cursor)
                .unwrap()
                .expect("firmware PCI device is missing");
            if devices.read(id, DeviceField::Name, 0).unwrap() == entry.name.as_bytes() {
                break id;
            }
            cursor = id;
        };
        let snapshot = devices.read(id, DeviceField::PciConfig, 0).unwrap();
        assert_eq!(
            u32::from_le_bytes(snapshot[entry.offset..entry.offset + 4].try_into().unwrap()),
            entry.value,
            "{} offset {}",
            entry.name,
            entry.offset
        );
    }
}

pub fn rollback(hardware: &mut HardwareInfo, expected: &[Expected]) {
    let invalid = hardware
        .devices
        .iter()
        .position(|device| !device.mmio_regions.is_empty())
        .unwrap();
    let range = hardware.devices[invalid].mmio_regions[0];
    hardware.devices[invalid].mmio_regions[0].end = range.start;
    let result = DeviceSet::initialize(hardware);
    hardware.devices[invalid].mmio_regions[0] = range;
    assert!(matches!(result, Err(DeviceError::InvalidArgument)));
    let host = hardware
        .devices
        .iter()
        .position(|device| {
            matches!(
                device.location,
                HardwareDeviceLocation::PciHostBridge { .. }
            )
        })
        .unwrap();
    for entry in expected {
        let mapping = map_bus(hardware, host, entry.bus);
        let base = entry.slot * 8 * 4096;
        assert_eq!(
            hardware::pci_config()
                .read32(&mapping, base + entry.offset)
                .unwrap(),
            entry.value
        );
        assert_eq!(
            hardware::pci_config().read16(&mapping, base + 4).unwrap() & 0x407,
            0x400
        );
    }
}

pub fn constrain(hardware: &mut HardwareInfo) {
    for device in &mut hardware.devices {
        if let HardwareDeviceLocation::PciHostBridge { windows, .. } = &mut device.location {
            let mut selected = false;
            windows.retain(|window| {
                if selected || window.space != PciAddressSpace::Memory32 || window.prefetchable {
                    return false;
                }
                selected = true;
                true
            });
            let window = windows.first_mut().unwrap();
            window.cpu_range.end = window.cpu_range.start + 0x100_0000;
        }
    }
}

fn map_bus(hardware: &mut HardwareInfo, host: usize, bus: u8) -> DeviceMapping {
    let HardwareDeviceLocation::PciHostBridge {
        ecam, first_bus, ..
    } = hardware.devices[host].location
    else {
        unreachable!()
    };
    let start = ecam.start + u64::from(bus - first_bus) * (1 << 20);
    let id = hardware.devices[host].instance_id;
    let region = hardware.devices[host].mmio_regions.len();
    hardware.devices[host].mmio_regions.push(PhysRange {
        start,
        end: start + (1 << 20),
    });
    let mapping = DeviceMapper::new(hardware).map(id, region).unwrap();
    hardware.devices[host].mmio_regions.pop();
    mapping
}
