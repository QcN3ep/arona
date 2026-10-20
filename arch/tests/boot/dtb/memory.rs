// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn reservations_cover_whole_pages() {
    let mut blob = DTB.to_vec();
    let reservation = fdt::Fdt::new_unaligned(&blob)
        .unwrap()
        .header()
        .memory_reserve_map_offset as usize;
    blob[reservation..reservation + 8].copy_from_slice(&0x80000001_u64.to_be_bytes());
    blob[reservation + 8..reservation + 16].copy_from_slice(&0x1ffe_u64.to_be_bytes());
    let tree = fdt::Fdt::new_unaligned(&blob).unwrap();
    let reg = tree
        .find_node("/reserved-memory/protected@80002000")
        .unwrap()
        .raw_property("reg")
        .unwrap();
    let offset = reg.value.as_ptr() as usize - blob.as_ptr() as usize;
    blob[offset..offset + 8].copy_from_slice(&0x80002fff_u64.to_be_bytes());
    blob[offset + 8..offset + 16].copy_from_slice(&2_u64.to_be_bytes());
    let mut hardware = parse_hardware(&blob);
    assert_eq!(
        hardware.memory.regions[0],
        MemoryRegion {
            range: PhysRange {
                start: 0x80000000,
                end: 0x80002000
            },
            state: MemoryRegionState::Reserved,
        }
    );
    let range = PhysRange {
        start: 0x80200001,
        end: 0x80201001,
    };
    hardware
        .memory
        .reserve(range, MemoryRegionState::BootReserved)
        .unwrap();
    assert!(hardware.memory.regions.contains(&MemoryRegion {
        range: PhysRange {
            start: 0x80200000,
            end: 0x80202000
        },
        state: MemoryRegionState::BootReserved
    }));
    assert!(hardware.memory.regions.contains(&MemoryRegion {
        range: PhysRange {
            start: 0x80002000,
            end: 0x80004000
        },
        state: MemoryRegionState::NoMap
    }));
    assert!(
        hardware
            .memory
            .reserve(
                PhysRange {
                    start: u64::MAX - 3,
                    end: u64::MAX
                },
                MemoryRegionState::BootReserved
            )
            .is_err()
    );
}
