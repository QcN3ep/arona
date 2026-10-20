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
fn rejects_broken_control_and_routing_descriptions() {
    for (original, path, property, cell, value) in [
        (DTB, "/memory@80000000", "reg", 1, 0x80000001),
        (
            LOONGARCH_DTB,
            "/msi@2ff00000",
            "loongson,msi-num-vecs",
            0,
            225,
        ),
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
