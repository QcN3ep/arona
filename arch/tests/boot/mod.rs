// SPDX-License-Identifier: MPL-2.0

mod dtb;

use arch::boot::prepare_boot_info;
use kernel::{
    CpuInfo, HardwareInfo, ImageSegment, KernelImage,
    memory::{MemoryRegion, MemoryRegionState, PhysRange},
};

const BOOT_CPU_ID: u64 = 7;

fn image(physical: PhysRange) -> KernelImage {
    let segment = |start, end| ImageSegment {
        physical: PhysRange { start, end },
        virtual_start: 0xffff_8000_0000_0000 + start as usize,
    };
    KernelImage {
        physical,
        text: segment(physical.start + 4096, physical.start + 2 * 4096),
        rodata: segment(physical.start + 2 * 4096, physical.start + 3 * 4096),
        data: segment(physical.start + 3 * 4096, physical.end),
        boot_tables: PhysRange {
            start: physical.end - 2 * 4096,
            end: physical.end - 4096,
        },
        boot_stack: PhysRange {
            start: physical.end - 4096,
            end: physical.end,
        },
    }
}

#[test]
fn preserves_image_layout_and_reserves_its_backing() {
    let mut hardware = HardwareInfo::new();
    hardware.cpus.push(CpuInfo {
        hardware_id: BOOT_CPU_ID,
    });
    hardware.memory.regions.push(MemoryRegion {
        range: PhysRange {
            start: 0x80000000,
            end: 0x88000000,
        },
        state: MemoryRegionState::Available,
    });
    let kernel_image = image(PhysRange {
        start: 0x80200000,
        end: 0x80300000,
    });
    let info = prepare_boot_info(BOOT_CPU_ID, kernel_image, &mut hardware).unwrap();
    assert_eq!(info.boot_cpu_id, BOOT_CPU_ID);
    assert_eq!(info.kernel_image, kernel_image);
    assert_eq!(
        hardware.memory.regions.as_slice(),
        &[
            MemoryRegion {
                range: PhysRange {
                    start: 0x80000000,
                    end: kernel_image.physical.start,
                },
                state: MemoryRegionState::Available,
            },
            MemoryRegion {
                range: kernel_image.physical,
                state: MemoryRegionState::Reserved,
            },
            MemoryRegion {
                range: PhysRange {
                    start: kernel_image.physical.end,
                    end: 0x88000000,
                },
                state: MemoryRegionState::Available,
            },
        ]
    );
    assert_eq!(
        prepare_boot_info(99, kernel_image, &mut hardware).unwrap_err(),
        "boot CPU is missing from the hardware description"
    );
    let no_map = PhysRange {
        start: 0x80002000,
        end: 0x80003000,
    };
    hardware
        .memory
        .reserve(no_map, MemoryRegionState::NoMap)
        .unwrap();
    let before = hardware.memory.clone();
    for range in [
        no_map,
        PhysRange {
            start: 0x90000000,
            end: 0x90001000,
        },
        PhysRange {
            start: 0x88000000 - 4096,
            end: 0x88000000 + 4096,
        },
    ] {
        assert_eq!(
            prepare_boot_info(
                BOOT_CPU_ID,
                KernelImage {
                    physical: range,
                    ..kernel_image
                },
                &mut hardware,
            )
            .unwrap_err(),
            "reservation is not contained in mappable RAM"
        );
        assert_eq!(hardware.memory, before);
    }
}
