// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

mod address_space;
mod direct_map;
mod frame;
mod frame_allocator;
mod heap_allocator;

use kernel::{
    hardware::{self, mmu::PAGE_SIZE},
    memory::{
        self, AddressSpace, Frame, MappingOptions, MappingRights, MemoryError, MemoryRegion,
        MemoryRegionState, PhysRange, VirtRange,
    },
};

use kernel::hardware::mmu::{PageMapping, PagePermissions, PageTableEntry};

const ADDRESS: usize = 0x1f_f000;
const PAGE: u64 = PAGE_SIZE as u64;

pub fn validate_regions(regions: &[MemoryRegion]) {
    let available = regions
        .iter()
        .find(|region| region.state == MemoryRegionState::Available)
        .unwrap();
    let start = available.range.start;
    for range in [
        PhysRange { start, end: start },
        PhysRange {
            start: start + PAGE,
            end: start,
        },
        PhysRange {
            start: start + 1,
            end: start + PAGE,
        },
    ] {
        assert_eq!(
            memory::initialize(&[MemoryRegion {
                range,
                ..*available
            }]),
            Err(MemoryError::InvalidRequest)
        );
    }
    assert_eq!(
        memory::initialize(&[*available, *available]),
        Err(MemoryError::InvalidRequest)
    );
    assert_eq!(memory::initialize(&[]), Err(MemoryError::NoMemory));
    let oversized = [*available; memory::MAX_MEMORY_REGIONS + 1];
    assert_eq!(
        memory::initialize(&oversized),
        Err(MemoryError::InvalidRequest)
    );
}

pub fn run(regions: &[MemoryRegion]) {
    let manager = memory::manager();
    assert_eq!(
        memory::initialize(regions),
        Err(MemoryError::AlreadyInitialized)
    );
    assert_eq!(
        manager.initialize_kernel_address_space(),
        Err(MemoryError::AlreadyInitialized)
    );
    assert_eq!(
        manager.initialize_direct_map(),
        Err(MemoryError::AlreadyInitialized)
    );
    direct_map::run(regions);
    frame_allocator::run(regions);
    frame::run();
    address_space::run();
    heap_allocator::run();
}

pub fn concurrent() {
    frame::concurrent();
}

fn range(start: usize, pages: usize) -> VirtRange {
    VirtRange {
        start,
        end: start + pages * PAGE_SIZE,
    }
}

fn mutable_code() -> MappingOptions {
    MappingOptions {
        permissions: PagePermissions::ReadWrite,
        maximum: MappingRights::READ | MappingRights::WRITE | MappingRights::EXECUTE,
        alignment: PAGE_SIZE,
    }
}

fn lookup(space: &AddressSpace, address: usize) -> Option<PageMapping> {
    let mmu = hardware::mmu();
    let layout = mmu.layout();
    let mut table = space.root();
    for level in (0..layout.levels).rev() {
        let shift = PAGE_SIZE.trailing_zeros() + layout.index_bits * level as u32;
        let slot = (address >> shift) & ((1 << layout.index_bits) - 1);
        match mmu.read_entry(table, slot, level).unwrap() {
            PageTableEntry::Empty => return None,
            PageTableEntry::Table(next) => table = next,
            PageTableEntry::Mapping(mut mapping) => {
                mapping.physical_address += (address & ((1usize << shift) - 1)) as u64;
                return Some(mapping);
            }
        }
    }
    panic!("missing leaf mapping");
}

fn first_frame(allocation: &memory::FrameAllocation) -> Frame {
    allocation.frames().next().unwrap()
}
