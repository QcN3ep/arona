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
fn rejects_invalid_device_properties() {
    for (original, path, property, cell, value) in [
        (DTB, "/soc@10000000/serial@0", "reg-shift", 0, 63),
        (DTB, "/soc@10000000/serial@0", "reg-shift", 0, 255),
        (DTB, "/soc@10000000/serial@0", "reg-io-width", 0, 0),
        (DTB, "/soc@10000000/serial@0", "reg-offset", 0, u32::MAX),
        (DTB, "/clock", "clock-frequency", 0, 0),
        (DTB, "/memory@80000000", "reg", 3, 0x8000001),
        (DTB, "/interrupt-controller@c000000", "riscv,ndev", 0, 0),
        (LOONGARCH_DTB, "/pcie@20000000", "interrupt-map", 3, 0),
        (LOONGARCH_DTB, "/pcie@20000000", "interrupt-map-mask", 3, 0),
        (LOONGARCH_DTB, "/pcie@20000000", "reg", 1, 0x20000001),
        (LOONGARCH_DTB, "/pcie@20000000", "ranges", 9, 0x40000001),
        (LOONGARCH_DTB, "/pcie@20000000", "ranges", 11, 0x40000001),
        (LOONGARCH_DTB, "/pcie@20000000", "ranges", 8, 1),
        (LOONGARCH_DTB, "/pcie@20000000", "interrupt-map", 10, 1),
        (LOONGARCH_DTB, "/pcie@20000000", "interrupt-map", 5, 6),
        (LOONGARCH_DTB, "/ged@100e001c", "reg-io-width", 0, 0),
        (
            LOONGARCH_DTB,
            "/msi@2ff00000",
            "loongson,msi-num-vecs",
            0,
            0,
        ),
        (
            LOONGARCH_DTB,
            "/platic@10000000",
            "loongson,pic-base-vec",
            0,
            193,
        ),
        (LOONGARCH_DTB, "/eiointc@1400", "interrupts", 0, 14),
        (LOONGARCH_DTB, "/flash@1c000000", "bank-width", 0, 3),
        (RISCV_DTB, "/cpus", "timebase-frequency", 0, 0),
        (
            RISCV_DTB,
            "/soc/interrupt-controller@c000000",
            "interrupts-extended",
            1,
            8,
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
            "{path}: {cell} = {value}"
        );
    }
    let mut blob = LOONGARCH_DTB.to_vec();
    let tree = fdt::Fdt::new_unaligned(&blob).unwrap();
    let reg = tree
        .find_node("/serial@1fe001e0")
        .unwrap()
        .raw_property("reg")
        .unwrap();
    let offset = reg.value.as_ptr() as usize - blob.as_ptr() as usize;
    let strings = tree.header().strings_offset as usize;
    let name = strings
        + blob[strings..]
            .windows(16)
            .position(|bytes| bytes == b"clock-frequency\0")
            .unwrap();
    // 复用已有属性的名字，为 UART 注入会溢出绝对地址的偏移。
    blob[name..name + 11].copy_from_slice(b"reg-offset\0");
    for (index, value) in [u32::MAX, u32::MAX - 3, 0, 3].into_iter().enumerate() {
        blob[offset + index * 4..offset + index * 4 + 4].copy_from_slice(&value.to_be_bytes());
    }
    let mut hardware = Box::new(HardwareInfo::new());
    assert_eq!(
        Dtb::from_bytes(&blob)
            .unwrap()
            .dump(&mut hardware)
            .unwrap_err()
            .to_string(),
        "invalid UART register layout or clock"
    );

    for (blob, kind) in [
        (LOONGARCH_DTB, HardwareDeviceKind::LOONGARCH_LS7A_RTC),
        (RISCV_DTB, HardwareDeviceKind::GOLDFISH_RTC),
    ] {
        let hardware = parse_hardware(blob);
        assert_eq!(
            hardware
                .devices
                .iter()
                .find(|device| device.kind == kind)
                .unwrap()
                .use_policy,
            DeviceUsePolicy::KernelReserved
        );
    }
}

#[test]
fn rejects_conflicting_device_resources() {
    for (original, path, property, values, expected) in [
        (
            DTB,
            "/soc@10000000/virtio@1000",
            "reg",
            [0, 0, 0, 0x1000].as_slice(),
            "device register ranges overlap",
        ),
        (
            DTB,
            "/soc@10000000/virtio@1000",
            "reg",
            [0, 0x100, 0, 0x100].as_slice(),
            "assignable device shares a resource page",
        ),
        (
            LOONGARCH_DTB,
            "/serial@1fe003e0",
            "reg",
            [0, 0x1fe002e0, 0, 0x100].as_slice(),
            "device register ranges overlap",
        ),
        (
            LOONGARCH_DTB,
            "/serial@1fe003e0",
            "interrupts",
            [3, 4].as_slice(),
            "shared IRQs are not supported",
        ),
    ] {
        let mut blob = original.to_vec();
        let tree = fdt::Fdt::new_unaligned(&blob).unwrap();
        let property = tree
            .find_node(path)
            .unwrap()
            .raw_property(property)
            .unwrap();
        let offset = property.value.as_ptr() as usize - blob.as_ptr() as usize;
        for (index, value) in values.iter().enumerate() {
            blob[offset + index * 4..offset + index * 4 + 4]
                .copy_from_slice(&u32::to_be_bytes(*value));
        }
        let mut hardware = Box::new(HardwareInfo::new());
        let error = Dtb::from_bytes(&blob)
            .unwrap()
            .dump(&mut hardware)
            .unwrap_err();
        assert_eq!(error.to_string(), expected, "{path}");
    }
}

#[test]
fn unsupported_optional_information_does_not_prevent_boot() {
    for (path, property, value) in [
        ("/poweroff", "regmap", 0xdead),
        ("/poweroff", "offset", 0xffff),
        ("/poweroff", "value", 0x100),
        ("/reboot", "regmap", 0xdead),
        ("/reboot", "offset", 0xffff),
        ("/reboot", "value", 0x100),
    ] {
        let mut blob = LOONGARCH_DTB.to_vec();
        let tree = fdt::Fdt::new_unaligned(&blob).unwrap();
        let property = tree
            .find_node(path)
            .unwrap()
            .raw_property(property)
            .unwrap();
        let offset = property.value.as_ptr() as usize - blob.as_ptr() as usize;
        blob[offset..offset + 4].copy_from_slice(&u32::to_be_bytes(value));
        let hardware = parse_hardware(&blob);
        let control = hardware
            .devices
            .iter()
            .find_map(|device| match device.configuration {
                DeviceConfiguration::Syscon(info) => Some(info),
                _ => None,
            })
            .unwrap();
        assert_eq!(control.poweroff.is_none(), path == "/poweroff");
        assert_eq!(control.reboot.is_none(), path == "/reboot");
    }
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

#[test]
fn rejects_broken_control_and_routing_descriptions() {
    for (original, path, property, cell, value) in [
        (DTB, "/soc@10000000/serial@0", "reg-shift", 0, 0),
        (DTB, "/soc@10000000/serial@0", "reg-shift", 0, 62),
        (DTB, "/soc@10000000/serial@0", "reg-shift", 0, 64),
        (DTB, "/memory@80000000", "reg", 1, 0x80000001),
        (LOONGARCH_DTB, "/ged@100e001c", "reg-io-width", 0, 4),
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
