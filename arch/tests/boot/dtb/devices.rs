// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn accepts_128_devices_and_rejects_overflow() {
    let header = *fdt::Fdt::new_unaligned(DTB).unwrap().header();
    let strings = &DTB[header.strings_offset as usize..];
    let name_offset = |name: &[u8]| {
        strings
            .windows(name.len())
            .position(|bytes| bytes == name)
            .unwrap() as u32
    };
    let insertion = (header.structs_offset + header.structs_size) as usize - 8;
    for count in [65, 128, 129] {
        let mut extra = Vec::new();
        for index in 0..count - 4 {
            let address = 0x90000000 + index * 4096;
            extra.extend_from_slice(&1_u32.to_be_bytes());
            extra.extend_from_slice(format!("virtio@{address:x}\0").as_bytes());
            while !extra.len().is_multiple_of(4) {
                extra.push(0);
            }
            for word in [3, 12, name_offset(b"compatible\0")] {
                extra.extend_from_slice(&word.to_be_bytes());
            }
            extra.extend_from_slice(b"virtio,mmio\0");
            for word in [3, 16, name_offset(b"reg\0"), 0, address, 0, 4096, 2] {
                extra.extend_from_slice(&word.to_be_bytes());
            }
        }
        let mut blob = DTB.to_vec();
        let size = extra.len() as u32;
        blob.splice(insertion..insertion, extra);
        for (offset, value) in [
            (4, blob.len() as u32),
            (12, header.strings_offset + size),
            (36, header.structs_size + size),
        ] {
            blob[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        }
        let mut hardware = Box::new(HardwareInfo::new());
        let (hardware, result) = std::thread::Builder::new()
            .stack_size(64 * 1024)
            .spawn(move || {
                let result = Dtb::from_bytes(&blob).unwrap().dump(&mut hardware);
                (hardware, result)
            })
            .unwrap()
            .join()
            .unwrap();
        if count <= 128 {
            result.unwrap();
            assert_eq!(hardware.devices.len(), count as usize);
        } else {
            assert_eq!(result.unwrap_err().to_string(), "too many devices");
        }
    }
}

#[test]
fn rejects_invalid_normalized_resources() {
    let mut hardware = parse_hardware(DTB);
    let uart = hardware
        .devices
        .iter()
        .position(|device| device.kind == HardwareDeviceKind::UART_16550)
        .unwrap();
    let DeviceConfiguration::Uart(mut serial) = hardware.devices[uart].configuration else {
        unreachable!()
    };
    hardware.devices[uart].mmio_regions[0] = PhysRange {
        start: u64::MAX - 3,
        end: u64::MAX,
    };
    assert_eq!(
        hardware.validate_resources(),
        Err("invalid UART register layout or clock")
    );
    hardware.devices[uart].mmio_regions[0] = PhysRange {
        start: 0,
        end: u64::MAX - 4095,
    };
    serial.register_width = 1;
    serial.register_offset = 0;
    for shift in [62, 63, 64, 255] {
        serial.register_shift = shift;
        hardware.devices[uart].configuration = DeviceConfiguration::Uart(serial);
        assert_eq!(
            hardware.validate_resources(),
            Err("invalid UART register layout or clock")
        );
    }
    let mut hardware = parse_hardware(LOONGARCH_DTB);
    hardware.pci.interrupts[0].pin = 0;
    hardware.pci.interrupts[0].pin_mask = 0;
    assert_eq!(
        hardware.validate_resources(),
        Err("invalid PCI interrupt route")
    );
    hardware.pci.interrupts[0].pin = 1;
    hardware.pci.interrupts[0].pin_mask = 7;
    let pci = hardware
        .devices
        .iter()
        .position(|device| device.kind == HardwareDeviceKind::PCI_HOST_BRIDGE)
        .unwrap();
    let HardwareDeviceLocation::PciHostBridge { ecam, .. } = &mut hardware.devices[pci].location
    else {
        unreachable!()
    };
    ecam.start += 1;
    ecam.end += 1;
    assert_eq!(
        hardware.validate_resources(),
        Err("invalid PCI ECAM bus range")
    );
    let HardwareDeviceLocation::PciHostBridge { ecam, windows, .. } =
        &mut hardware.devices[pci].location
    else {
        unreachable!()
    };
    ecam.start -= 1;
    ecam.end -= 1;
    windows[0].bus_address += 1;
    assert_eq!(
        hardware.validate_resources(),
        Err("PCI memory window is not page-aligned")
    );
}

#[test]
fn rejects_broken_control_and_routing_descriptions() {
    for (original, path, property, cell, value) in [
        (DTB, "/soc@10000000/serial@0", "reg-shift", 0, 0),
        (DTB, "/soc@10000000/serial@0", "reg-shift", 0, 62),
        (DTB, "/soc@10000000/serial@0", "reg-shift", 0, 64),
        (DTB, "/memory@80000000", "reg", 1, 0x80000001),
        (
            LOONGARCH_DTB,
            "/msi@2ff00000",
            "loongson,msi-num-vecs",
            0,
            225,
        ),
        (LOONGARCH_DTB, "/pcie@20000000", "msi-map", 3, 65537),
        (LOONGARCH_DTB, "/pcie@20000000", "interrupt-map", 4, 0xdead),
        (
            RISCV_DTB,
            "/soc/interrupt-controller@c000000",
            "interrupts-extended",
            0,
            0xdead,
        ),
    ] {
        let mut blob = original.to_vec();
        let tree = fdt::Fdt::new_unaligned(&blob).unwrap();
        let property = tree
            .find_node(path)
            .unwrap()
            .raw_property(property)
            .unwrap();
        let offset = property.value.as_ptr() as usize - blob.as_ptr() as usize + cell * 4;
        blob[offset..offset + 4].copy_from_slice(&u32::to_be_bytes(value));
        let mut hardware = Box::new(HardwareInfo::new());
        assert!(
            Dtb::from_bytes(&blob).unwrap().dump(&mut hardware).is_err(),
            "{path}"
        );
    }
}

#[test]
fn unsupported_optional_information_does_not_prevent_boot() {
    for stdout in [
        b"missing:115200n8r".as_slice(),
        b"serial0:115200x8r".as_slice(),
    ] {
        let mut blob = DTB.to_vec();
        let tree = fdt::Fdt::new_unaligned(&blob).unwrap();
        let property = tree
            .find_node("/chosen")
            .unwrap()
            .raw_property("stdout-path")
            .unwrap();
        let offset = property.value.as_ptr() as usize - blob.as_ptr() as usize;
        blob[offset..offset + stdout.len()].copy_from_slice(stdout);
        let hardware = parse_hardware(&blob);
        if stdout.starts_with(b"missing") {
            assert_eq!(hardware.console, ConsoleInfo::Unspecified);
        } else {
            assert!(matches!(
                hardware.console,
                ConsoleInfo::Uart { settings: None, .. }
            ));
        }
    }
}
