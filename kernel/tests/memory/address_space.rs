// SPDX-License-Identifier: MPL-2.0

use super::*;
use kernel::memory::{AddressSpace, MappingOptions, MappingRights, USER_ADDRESS_RANGE, VirtRange};

const ADDRESS: usize = 0x1f_f000;

fn setup(frames: u64) -> &'static MemoryManager {
    let manager = initialize(&memory(frames));
    assert!(matches!(
        AddressSpace::new(),
        Err(MemoryError::NotInitialized)
    ));
    manager.initialize_kernel_address_space().unwrap();
    assert_eq!(
        manager.initialize_kernel_address_space(),
        Err(MemoryError::AlreadyInitialized)
    );
    manager
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

#[test]
fn mappings_retain_allocations_and_support_partial_changes() {
    if isolated("address_space::mappings_retain_allocations_and_support_partial_changes") {
        return;
    }
    let manager = setup(5);
    let initial = manager.stats();
    let space = AddressSpace::new().unwrap();
    let empty = manager.stats();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 5)
        .unwrap();
    let frames: Vec<_> = allocation.frames().collect();
    let mapped = space
        .map_at(ADDRESS, &allocation, 1..4, mutable_code())
        .unwrap();
    assert_eq!(mapped, range(ADDRESS, 3));
    assert_eq!(allocation.info().mappings, 3);
    for index in 0..3 {
        assert_eq!(
            space.translate(ADDRESS + index * PAGE_SIZE + 17).unwrap(),
            Some(frames[index + 1].address() + 17)
        );
    }
    let first = space.query(ADDRESS).unwrap().unwrap();
    drop(allocation);
    assert_eq!(manager.stats().user.free_frames, 0);
    space
        .protect(range(ADDRESS + PAGE_SIZE, 1), PagePermissions::ReadOnly)
        .unwrap();
    assert_eq!(space.query(ADDRESS).unwrap(), Some(first));
    let changed = space.query(ADDRESS + PAGE_SIZE).unwrap().unwrap();
    assert_ne!(changed.generation, first.generation);
    assert_eq!(
        changed.mapping.unwrap().attributes.permissions,
        PagePermissions::ReadOnly
    );
    space.unmap(range(ADDRESS + PAGE_SIZE, 1)).unwrap();
    assert_eq!(space.translate(ADDRESS + PAGE_SIZE).unwrap(), None);
    assert_eq!(manager.stats().user.free_frames, 0);
    assert_eq!(space.unmap(mapped), Err(MemoryError::NotMapped));
    assert_eq!(space.query(ADDRESS).unwrap(), Some(first));
    space.unmap(range(ADDRESS, 1)).unwrap();
    space.unmap(range(ADDRESS + 2 * PAGE_SIZE, 1)).unwrap();
    assert_eq!(manager.stats(), empty);
    drop(space);
    assert_eq!(manager.stats(), initial);
}

#[test]
fn reservations_find_aligned_gaps_and_reclaim_their_metadata() {
    if isolated("address_space::reservations_find_aligned_gaps_and_reclaim_their_metadata") {
        return;
    }
    let manager = setup(1);
    let space = AddressSpace::new().unwrap();
    let initial = manager.stats();
    space.reserve_at(USER_ADDRESS_RANGE).unwrap();
    assert_eq!(
        space.reserve(PAGE_SIZE, PAGE_SIZE),
        Err(MemoryError::NoMemory)
    );
    assert_eq!(space.unmap(USER_ADDRESS_RANGE), Err(MemoryError::InUse));
    assert_eq!(
        space.protect(USER_ADDRESS_RANGE, PagePermissions::ReadOnly),
        Err(MemoryError::InUse)
    );
    space.unreserve(USER_ADDRESS_RANGE).unwrap();
    let mut occupied = BTreeMap::new();
    let mut random = 0x741a_9155u64;
    for step in 0..3000 {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        let slot = ((random >> 32) % 512) as usize;
        let extent = range(USER_ADDRESS_RANGE.start + slot * PAGE_SIZE, 1);
        if occupied.remove(&extent.start).is_some() {
            space.unreserve(extent).unwrap();
        } else {
            space.reserve_at(extent).unwrap();
            occupied.insert(extent.start, extent);
        }
        if step % 17 == 0 {
            let length = (1 + (random as usize % 5)) * PAGE_SIZE;
            let alignment = PAGE_SIZE << (random as usize % 4);
            let mut candidate = (USER_ADDRESS_RANGE.start + alignment - 1) & !(alignment - 1);
            for extent in occupied.values() {
                if candidate + length <= extent.start {
                    break;
                }
                if candidate < extent.end {
                    candidate = (extent.end + alignment - 1) & !(alignment - 1);
                }
            }
            let reserved = space.reserve(length, alignment).unwrap();
            assert_eq!(
                reserved,
                VirtRange {
                    start: candidate,
                    end: candidate + length
                }
            );
            assert_eq!(space.query(candidate).unwrap().unwrap().mapping, None);
            space.unreserve(reserved).unwrap();
        }
    }
    for extent in occupied.values() {
        space.unreserve(*extent).unwrap();
    }
    assert_eq!(manager.stats(), initial);
}

#[test]
fn kernel_subtrees_are_shared_and_user_subtrees_are_private() {
    if isolated("address_space::kernel_subtrees_are_shared_and_user_subtrees_are_private") {
        return;
    }
    let manager = setup(2);
    let initial = manager.stats();
    let first = AddressSpace::new().unwrap();
    let second = AddressSpace::new().unwrap();
    let layout = MMU.layout();
    let kernel = manager.kernel_address_space();
    for slot in (1 << (layout.index_bits - 1))..(1 << layout.index_bits) {
        let entry = MMU
            .read_entry(kernel.root(), slot, layout.levels - 1)
            .unwrap();
        assert!(matches!(entry, PageTableEntry::Table(_)));
        for space in [&first, &second] {
            assert_eq!(
                MMU.read_entry(space.root(), slot, layout.levels - 1)
                    .unwrap(),
                entry
            );
        }
    }
    let bits = 12 + layout.index_bits * layout.levels as u32;
    let kernel_base = (usize::MAX << (bits - 1))
        + (1usize << (12 + layout.index_bits * (layout.levels - 1) as u32));
    let allocation = manager
        .allocate(FrameOwner::Kernel, FramePurpose::Data, 2)
        .unwrap();
    let frames: Vec<_> = allocation.frames().collect();
    let mapped = kernel
        .map_at(
            kernel_base,
            &allocation,
            0..2,
            MappingOptions::new(PagePermissions::ReadWrite),
        )
        .unwrap();
    assert_eq!(allocation.info().mappings, 2);
    for space in [&first, &second] {
        assert_eq!(
            MMU.lookup(space.root(), kernel_base)
                .unwrap()
                .physical_address,
            frames[0].address()
        );
        assert_eq!(space.query(kernel_base), Err(MemoryError::InvalidRequest));
        assert_eq!(
            space.translate(kernel_base + 1),
            Err(MemoryError::InvalidRequest)
        );
        assert_eq!(space.unmap(mapped), Err(MemoryError::InvalidRequest));
    }
    let user = manager
        .allocate(FrameOwner::User(7), FramePurpose::Data, 2)
        .unwrap();
    let user_frames: Vec<_> = user.frames().collect();
    let options = MappingOptions::new(PagePermissions::ReadWrite);
    first.map_at(ADDRESS, &user, 0..1, options).unwrap();
    second.map_at(ADDRESS, &user, 1..2, options).unwrap();
    assert_eq!(
        first.translate(ADDRESS).unwrap(),
        Some(user_frames[0].address())
    );
    assert_eq!(
        second.translate(ADDRESS).unwrap(),
        Some(user_frames[1].address())
    );
    kernel.protect(mapped, PagePermissions::ReadOnly).unwrap();
    assert_eq!(
        MMU.lookup(first.root(), kernel_base)
            .unwrap()
            .attributes
            .permissions,
        PagePermissions::ReadOnly
    );
    drop(first);
    assert_eq!(allocation.info().mappings, 2);
    assert_eq!(
        MMU.lookup(second.root(), kernel_base + PAGE_SIZE)
            .unwrap()
            .physical_address,
        frames[1].address()
    );
    kernel.unmap(mapped).unwrap();
    assert_eq!(MMU.lookup(second.root(), kernel_base), None);
    kernel
        .map_at(kernel_base, &allocation, 0..1, options)
        .unwrap();
    assert_eq!(
        MMU.lookup(second.root(), kernel_base)
            .unwrap()
            .physical_address,
        frames[0].address()
    );
    kernel.unmap(range(kernel_base, 1)).unwrap();
    drop(second);
    drop(user);
    drop(allocation);
    assert_eq!(manager.stats(), initial);
}

#[test]
fn map_failures_restore_records_page_tables_and_frame_references() {
    if isolated("address_space::map_failures_restore_records_page_tables_and_frame_references") {
        return;
    }
    let manager = setup(3);
    let space = AddressSpace::new().unwrap();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 3)
        .unwrap();
    let initial = manager.stats();
    for writes in 0..MMU.layout().levels as u64 + 3 {
        MMU.writes_until_failure.store(writes, Ordering::Relaxed);
        assert_eq!(
            space.map_at(ADDRESS, &allocation, 0..3, mutable_code()),
            Err(MemoryError::Mmu(MmuError::InvalidTable))
        );
        assert_eq!(allocation.info().mappings, 0);
        assert_eq!(manager.stats(), initial);
        for page in 0..3 {
            assert_eq!(space.query(ADDRESS + page * PAGE_SIZE).unwrap(), None);
        }
    }
    for zeros in 0..MMU.layout().levels as u64 + 1 {
        MMU.zeros_until_failure.store(zeros, Ordering::Relaxed);
        assert!(
            space
                .map_at(ADDRESS, &allocation, 0..3, mutable_code())
                .is_err()
        );
        assert_eq!(allocation.info().mappings, 0);
        assert_eq!(manager.stats(), initial);
    }
    let mut executable = mutable_code();
    executable.permissions = PagePermissions::ReadExecute;
    MMU.fail_synchronization.store(true, Ordering::Relaxed);
    assert!(
        space
            .map_at(ADDRESS, &allocation, 0..3, executable)
            .is_err()
    );
    assert_eq!(allocation.info().mappings, 0);
    assert_eq!(manager.stats(), initial);
    let mapped = space
        .map_at(ADDRESS, &allocation, 0..3, mutable_code())
        .unwrap();
    space.unmap(mapped).unwrap();
    assert_eq!(manager.stats(), initial);
}

#[test]
fn unmap_failures_restore_detached_branches_and_every_mapping() {
    if isolated("address_space::unmap_failures_restore_detached_branches_and_every_mapping") {
        return;
    }
    let manager = setup(3);
    let space = AddressSpace::new().unwrap();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 3)
        .unwrap();
    let empty = manager.stats();
    let mapped = space
        .map_at(ADDRESS, &allocation, 0..3, mutable_code())
        .unwrap();
    let initial = manager.stats();
    let original: Vec<_> = (0..3)
        .map(|page| space.query(ADDRESS + page * PAGE_SIZE).unwrap())
        .collect();
    for writes in 0..3 + MMU.layout().levels as u64 {
        MMU.writes_until_failure.store(writes, Ordering::Relaxed);
        assert_eq!(
            space.unmap(mapped),
            Err(MemoryError::Mmu(MmuError::InvalidTable))
        );
        assert_eq!(allocation.info().mappings, 3);
        assert_eq!(manager.stats(), initial);
        for (page, before) in original.iter().enumerate() {
            assert_eq!(space.query(ADDRESS + page * PAGE_SIZE).unwrap(), *before);
        }
    }
    space.unmap(mapped).unwrap();
    assert_eq!(allocation.info().mappings, 0);
    assert_eq!(manager.stats(), empty);
}

#[test]
fn protection_failures_restore_permissions_generations_and_access_counts() {
    if isolated(
        "address_space::protection_failures_restore_permissions_generations_and_access_counts",
    ) {
        return;
    }
    let manager = setup(3);
    let space = AddressSpace::new().unwrap();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 3)
        .unwrap();
    let mapped = space
        .map_at(ADDRESS, &allocation, 0..3, mutable_code())
        .unwrap();
    let original: Vec<_> = (0..3)
        .map(|page| space.query(ADDRESS + page * PAGE_SIZE).unwrap())
        .collect();
    let initial = manager.stats();
    for writes in 0..6 {
        MMU.writes_until_failure.store(writes, Ordering::Relaxed);
        assert_eq!(
            space.protect(mapped, PagePermissions::ReadExecute),
            Err(MemoryError::Mmu(MmuError::InvalidTable))
        );
        for (page, before) in original.iter().enumerate() {
            assert_eq!(space.query(ADDRESS + page * PAGE_SIZE).unwrap(), *before);
        }
        for frame in allocation.frames() {
            drop(
                allocation
                    .access(frame, ReferenceKind::Temporary, FrameAccess::Write)
                    .unwrap(),
            );
        }
        assert_eq!(manager.stats(), initial);
    }
    MMU.fail_synchronization.store(true, Ordering::Relaxed);
    assert!(space.protect(mapped, PagePermissions::ReadExecute).is_err());
    for (page, before) in original.iter().enumerate() {
        assert_eq!(space.query(ADDRESS + page * PAGE_SIZE).unwrap(), *before);
    }
    space.protect(mapped, PagePermissions::ReadExecute).unwrap();
    for frame in allocation.frames() {
        assert!(
            allocation
                .access(frame, ReferenceKind::Temporary, FrameAccess::Write)
                .is_err()
        );
    }
    MMU.zeros_until_failure.store(0, Ordering::Relaxed);
    space.protect(mapped, PagePermissions::ReadOnly).unwrap();
    space.unmap(mapped).unwrap();
    assert_eq!(MMU.zeros_until_failure.load(Ordering::Relaxed), 0);
    MMU.zeros_until_failure.store(NO_FAILURE, Ordering::Relaxed);
}

#[test]
fn physical_aliases_change_permissions_as_one_range() {
    if isolated("address_space::physical_aliases_change_permissions_as_one_range") {
        return;
    }
    let manager = setup(1);
    let space = AddressSpace::new().unwrap();
    let other = AddressSpace::new().unwrap();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 1)
        .unwrap();
    let frame = allocation.frames().next().unwrap();
    space
        .map_at(ADDRESS, &allocation, 0..1, mutable_code())
        .unwrap();
    space
        .map_at(ADDRESS + PAGE_SIZE, &allocation, 0..1, mutable_code())
        .unwrap();
    let aliases = range(ADDRESS, 2);
    let dma = allocation
        .access(frame, ReferenceKind::Dma, FrameAccess::Write)
        .unwrap();
    assert_eq!(
        space.protect(aliases, PagePermissions::ReadExecute),
        Err(MemoryError::AccessConflict)
    );
    drop(dma);
    space
        .protect(aliases, PagePermissions::ReadExecute)
        .unwrap();
    other
        .map_at(
            ADDRESS,
            &allocation,
            0..1,
            MappingOptions::new(PagePermissions::ReadExecute),
        )
        .unwrap();
    assert_eq!(
        space.protect(aliases, PagePermissions::ReadWrite),
        Err(MemoryError::AccessConflict)
    );
    other.unmap(range(ADDRESS, 1)).unwrap();
    space.protect(aliases, PagePermissions::ReadWrite).unwrap();
    assert_eq!(allocation.info().mappings, 2);
}

#[test]
fn invalid_ranges_authority_and_permission_ceilings_leave_mappings_unchanged() {
    if isolated(
        "address_space::invalid_ranges_authority_and_permission_ceilings_leave_mappings_unchanged",
    ) {
        return;
    }
    let manager = setup(2);
    let space = AddressSpace::new().unwrap();
    for address in [
        0,
        USER_ADDRESS_RANGE.start - 1,
        USER_ADDRESS_RANGE.end,
        usize::MAX,
    ] {
        assert_eq!(space.query(address), Err(MemoryError::InvalidRequest));
        assert_eq!(space.translate(address), Err(MemoryError::InvalidRequest));
    }
    let kernel_space = manager.kernel_address_space();
    assert_eq!(
        kernel_space.query(ADDRESS),
        Err(MemoryError::InvalidRequest)
    );
    assert_eq!(
        kernel_space.translate(ADDRESS),
        Err(MemoryError::InvalidRequest)
    );
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 2)
        .unwrap();
    let kernel = manager
        .allocate(FrameOwner::Kernel, FramePurpose::Data, 1)
        .unwrap();
    let options = MappingOptions::new(PagePermissions::ReadWrite);
    let initial = manager.stats();
    for address in [
        0,
        USER_ADDRESS_RANGE.start - PAGE_SIZE,
        ADDRESS + 1,
        USER_ADDRESS_RANGE.end,
        usize::MAX - PAGE_SIZE + 1,
    ] {
        assert!(space.map_at(address, &allocation, 0..1, options).is_err());
    }
    assert!(space.map_at(ADDRESS, &allocation, 0..0, options).is_err());
    assert!(space.map_at(ADDRESS, &allocation, 0..3, options).is_err());
    assert!(space.map_at(ADDRESS, &kernel, 0..1, options).is_err());
    for alignment in [0, 1, PAGE_SIZE + 1, usize::MAX] {
        assert!(
            space
                .map(
                    &allocation,
                    0..1,
                    MappingOptions {
                        alignment,
                        ..options
                    }
                )
                .is_err()
        );
    }
    assert_eq!(manager.stats(), initial);
    space.map_at(ADDRESS, &allocation, 0..1, options).unwrap();
    space
        .map_at(
            ADDRESS + PAGE_SIZE,
            &allocation,
            1..2,
            MappingOptions::new(PagePermissions::ReadOnly),
        )
        .unwrap();
    let first = space.query(ADDRESS).unwrap();
    assert_eq!(
        space.protect(range(ADDRESS, 2), PagePermissions::ReadWrite),
        Err(MemoryError::AccessConflict)
    );
    assert_eq!(
        space.protect(range(ADDRESS, 1), PagePermissions::ReadExecute),
        Err(MemoryError::AccessConflict)
    );
    assert_eq!(space.query(ADDRESS).unwrap(), first);
    assert_eq!(space.unmap(range(ADDRESS, 3)), Err(MemoryError::NotMapped));
    assert_eq!(space.query(ADDRESS).unwrap(), first);
}

#[test]
fn failed_root_creation_and_kernel_pool_exhaustion_do_not_leak() {
    if isolated("address_space::failed_root_creation_and_kernel_pool_exhaustion_do_not_leak") {
        return;
    }
    let manager = initialize(&memory(3));
    let initial = manager.stats();
    for writes in [0, 32, 255, 256] {
        MMU.writes_until_failure.store(writes, Ordering::Relaxed);
        assert_eq!(
            manager.initialize_kernel_address_space(),
            Err(MemoryError::Mmu(MmuError::InvalidTable))
        );
        assert_eq!(manager.stats(), initial);
    }
    manager.initialize_kernel_address_space().unwrap();
    let initialized = manager.stats();
    for writes in [0, 127, 255] {
        MMU.writes_until_failure.store(writes, Ordering::Relaxed);
        assert!(matches!(
            AddressSpace::new(),
            Err(MemoryError::Mmu(MmuError::InvalidTable))
        ));
        assert_eq!(manager.stats(), initialized);
    }
    let space = AddressSpace::new().unwrap();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 3)
        .unwrap();
    let pool = manager
        .allocate(
            FrameOwner::Kernel,
            FramePurpose::Data,
            manager.stats().kernel.free_frames - 1,
        )
        .unwrap();
    let exhausted = manager.stats();
    assert_eq!(
        space.map_at(ADDRESS, &allocation, 0..3, mutable_code()),
        Err(MemoryError::NoMemory)
    );
    assert_eq!(manager.stats(), exhausted);
    assert_eq!(allocation.info().mappings, 0);
    drop(pool);
    let reference = allocation.reference(ReferenceKind::Temporary).unwrap();
    drop(allocation);
    let mapped = space
        .map_reference(reference, 0..3, mutable_code())
        .unwrap();
    space.unmap(mapped).unwrap();
    assert_eq!(manager.stats().user.free_frames, 3);
}

#[test]
fn mapping_record_limit_counts_reservations_and_releases_failed_or_destroyed_records() {
    if isolated(
        "address_space::mapping_record_limit_counts_reservations_and_releases_failed_or_destroyed_records",
    ) {
        return;
    }
    let manager = setup(1);
    let initial = manager.stats();
    let first = AddressSpace::new().unwrap();
    let second = AddressSpace::new().unwrap();
    let third = AddressSpace::new().unwrap();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 1)
        .unwrap();
    let options = MappingOptions::new(PagePermissions::ReadOnly);
    let limit = kernel::memory::MAX_USER_MAPPING_RECORDS;
    for index in 0..limit - 1 {
        let address = USER_ADDRESS_RANGE.start + index / 2 * PAGE_SIZE;
        if index % 2 == 0 {
            first.map_at(address, &allocation, 0..1, options).unwrap();
        } else {
            second.reserve_at(range(address, 1)).unwrap();
        }
    }
    for reservation in [true, false] {
        MMU.zeros_until_failure.store(0, Ordering::Relaxed);
        let result = if reservation {
            third.reserve_at(range(ADDRESS, 1))
        } else {
            third
                .map_at(ADDRESS, &allocation, 0..1, options)
                .map(|_| ())
        };
        assert_eq!(result, Err(MemoryError::Mmu(MmuError::InvalidTable)));
    }
    let address = USER_ADDRESS_RANGE.start + (limit / 2 - 1) * PAGE_SIZE;
    second.reserve_at(range(address, 1)).unwrap();
    let beyond = address + PAGE_SIZE;
    assert_eq!(
        first.map_at(beyond, &allocation, 0..1, options),
        Err(MemoryError::NoMemory)
    );
    assert_eq!(
        second.reserve_at(range(beyond, 1)),
        Err(MemoryError::NoMemory)
    );
    assert_eq!(
        third.reserve(PAGE_SIZE, PAGE_SIZE),
        Err(MemoryError::NoMemory)
    );
    first.unmap(range(USER_ADDRESS_RANGE.start, 1)).unwrap();
    second.reserve_at(range(beyond, 1)).unwrap();
    second
        .unreserve(range(USER_ADDRESS_RANGE.start, 1))
        .unwrap();
    first
        .map_at(USER_ADDRESS_RANGE.start, &allocation, 0..1, options)
        .unwrap();
    drop(second);
    for index in 0..limit / 2 {
        third
            .reserve_at(range(USER_ADDRESS_RANGE.start + index * PAGE_SIZE, 1))
            .unwrap();
    }
    assert_eq!(
        third.reserve_at(range(beyond, 1)),
        Err(MemoryError::NoMemory)
    );
    drop(first);
    for index in 0..limit / 2 {
        third
            .map_at(beyond + index * PAGE_SIZE, &allocation, 0..1, options)
            .unwrap();
    }
    assert_eq!(
        third.reserve(PAGE_SIZE, PAGE_SIZE),
        Err(MemoryError::NoMemory)
    );
    drop(third);
    assert_eq!(allocation.info().mappings, 0);
    drop(allocation);
    assert_eq!(manager.stats(), initial);
}
