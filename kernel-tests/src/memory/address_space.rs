// SPDX-License-Identifier: MPL-2.0

use kernel::memory::{FrameAccess, FrameOwner, FramePurpose, ReferenceKind, USER_ADDRESS_RANGE};

use super::*;

pub(super) fn run() {
    mappings();
    reservations();
    shared_kernel();
    permissions();
    invalid_requests();
    exhaustion();
}

fn mappings() {
    let manager = memory::manager();
    manager.reclaim_heap();
    let initial = manager.stats();
    let space = AddressSpace::new().unwrap();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 5)
        .unwrap();
    let mut frames = allocation.frames();
    let identifiers: [_; 5] = core::array::from_fn(|_| frames.next().unwrap());
    let mapped = space
        .map_at(ADDRESS, &allocation, 1..4, mutable_code())
        .unwrap();
    assert_eq!(mapped, range(ADDRESS, 3));
    assert_eq!(allocation.info().mappings, 3);
    for index in 0..3 {
        assert_eq!(
            space.translate(ADDRESS + index * PAGE_SIZE + 17).unwrap(),
            Some(identifiers[index + 1].address() + 17)
        );
    }
    let first = space.query(ADDRESS).unwrap().unwrap();
    drop(allocation);
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
    assert_eq!(space.unmap(mapped), Err(MemoryError::NotMapped));
    assert_eq!(space.query(ADDRESS).unwrap(), Some(first));
    space.unmap(range(ADDRESS, 1)).unwrap();
    space.unmap(range(ADDRESS + 2 * PAGE_SIZE, 1)).unwrap();
    drop(space);
    manager.reclaim_heap();
    assert_eq!(manager.stats(), initial);
}

fn reservations() {
    let manager = memory::manager();
    manager.reclaim_heap();
    let initial = manager.stats();
    let space = AddressSpace::new().unwrap();
    let other = AddressSpace::new().unwrap();
    other.reserve_at(USER_ADDRESS_RANGE).unwrap();
    space.reserve_at(USER_ADDRESS_RANGE).unwrap();
    assert_eq!(
        space.reserve(PAGE_SIZE, PAGE_SIZE),
        Err(MemoryError::NoMemory)
    );
    assert_eq!(space.unmap(USER_ADDRESS_RANGE), Err(MemoryError::InUse));
    space.unreserve(USER_ADDRESS_RANGE).unwrap();
    let occupied = [(0, 3), (8, 2), (16, 5)]
        .map(|(offset, pages)| range(USER_ADDRESS_RANGE.start + offset * PAGE_SIZE, pages));
    for index in [2, 0, 1] {
        space.reserve_at(occupied[index]).unwrap();
    }
    assert_eq!(
        space.reserve_at(occupied[0]),
        Err(MemoryError::AlreadyMapped)
    );
    for (pages, alignment) in [(1, PAGE_SIZE), (4, 4 * PAGE_SIZE), (8, 16 * PAGE_SIZE)] {
        let reserved = space.reserve(pages * PAGE_SIZE, alignment).unwrap();
        assert!(reserved.start.is_multiple_of(alignment));
        assert_eq!(reserved.end - reserved.start, pages * PAGE_SIZE);
        assert!(
            USER_ADDRESS_RANGE.start <= reserved.start && reserved.end <= USER_ADDRESS_RANGE.end
        );
        assert!(occupied.iter().all(|range| !range.overlaps(reserved)));
        assert!(
            space
                .query(reserved.start)
                .unwrap()
                .unwrap()
                .mapping
                .is_none()
        );
        space.unreserve(reserved).unwrap();
    }
    for range in occupied {
        assert!(space.query(range.start).unwrap().is_some());
        space.unreserve(range).unwrap();
    }
    drop((space, other));
    manager.reclaim_heap();
    assert_eq!(manager.stats(), initial);
}

fn shared_kernel() {
    let manager = memory::manager();
    let kernel = manager.kernel_address_space();
    let first = AddressSpace::new().unwrap();
    let second = AddressSpace::new().unwrap();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 2)
        .unwrap();
    let frame = first_frame(&allocation);
    let mapped = kernel.map(&allocation, 0..1, mutable_code()).unwrap();
    for space in [&first, &second] {
        let mapping = lookup(space, mapped.start).unwrap();
        assert_eq!(mapping.physical_address, frame.address());
        assert!(!mapping.attributes.user_accessible);
        assert_eq!(space.query(mapped.start), Err(MemoryError::InvalidRequest));
        assert_eq!(
            space.translate(mapped.start),
            Err(MemoryError::InvalidRequest)
        );
        assert_eq!(space.unmap(mapped), Err(MemoryError::InvalidRequest));
    }
    kernel.protect(mapped, PagePermissions::ReadOnly).unwrap();
    for space in [&first, &second] {
        assert_eq!(
            lookup(space, mapped.start).unwrap().attributes.permissions,
            PagePermissions::ReadOnly
        );
    }
    first
        .map_at(ADDRESS, &allocation, 1..2, mutable_code())
        .unwrap();
    assert!(lookup(&first, ADDRESS).is_some());
    assert!(lookup(&second, ADDRESS).is_none());
    kernel.unmap(mapped).unwrap();
    assert!(lookup(&first, mapped.start).is_none());
    assert!(lookup(&second, mapped.start).is_none());
}

fn permissions() {
    let manager = memory::manager();
    let space = AddressSpace::new().unwrap();
    let other = AddressSpace::new().unwrap();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 2)
        .unwrap();
    let frame = first_frame(&allocation);
    space
        .map_at(ADDRESS, &allocation, 0..1, mutable_code())
        .unwrap();
    space
        .map_at(ADDRESS + PAGE_SIZE, &allocation, 0..1, mutable_code())
        .unwrap();
    other
        .map_at(ADDRESS, &allocation, 0..1, mutable_code())
        .unwrap();
    let aliases = range(ADDRESS, 2);
    let before = space.query(ADDRESS).unwrap();
    assert_eq!(
        space.protect(aliases, PagePermissions::ReadExecute),
        Err(MemoryError::AccessConflict)
    );
    assert_eq!(space.query(ADDRESS).unwrap(), before);
    other
        .protect(range(ADDRESS, 1), PagePermissions::ReadOnly)
        .unwrap();
    space
        .protect(aliases, PagePermissions::ReadExecute)
        .unwrap();
    assert!(matches!(
        allocation.access(frame, ReferenceKind::Dma, FrameAccess::Write),
        Err(MemoryError::AccessConflict)
    ));
    assert_eq!(
        other.protect(range(ADDRESS, 1), PagePermissions::ReadWrite),
        Err(MemoryError::AccessConflict)
    );
    for address in [ADDRESS, ADDRESS + PAGE_SIZE] {
        assert_eq!(
            space
                .query(address)
                .unwrap()
                .unwrap()
                .mapping
                .unwrap()
                .attributes
                .permissions,
            PagePermissions::ReadExecute
        );
    }
    space.unmap(aliases).unwrap();
    other
        .protect(range(ADDRESS, 1), PagePermissions::ReadWrite)
        .unwrap();
    let reference = allocation.reference(ReferenceKind::Temporary).unwrap();
    drop(allocation);
    let mapping = space
        .map_reference(reference, 1..2, mutable_code())
        .unwrap();
    space.unmap(mapping).unwrap();
}

fn invalid_requests() {
    let manager = memory::manager();
    let space = AddressSpace::new().unwrap();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 2)
        .unwrap();
    let kernel = manager
        .allocate(FrameOwner::Kernel, FramePurpose::Data, 1)
        .unwrap();
    let options = MappingOptions::new(PagePermissions::ReadWrite);
    let initial = manager.stats();
    for address in [0, USER_ADDRESS_RANGE.end, usize::MAX - PAGE_SIZE + 1] {
        assert!(space.map_at(address, &allocation, 0..1, options).is_err());
        assert_eq!(space.query(address), Err(MemoryError::InvalidRequest));
    }
    assert!(
        space
            .map_at(ADDRESS + 1, &allocation, 0..1, options)
            .is_err()
    );
    for frames in [0..0, 0..3] {
        assert!(space.map_at(ADDRESS, &allocation, frames, options).is_err());
    }
    assert!(space.map_at(ADDRESS, &kernel, 0..1, options).is_err());
    for alignment in [0, 1, PAGE_SIZE + 1] {
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
    assert_eq!(allocation.info().mappings, 0);
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
    let before = space.query(ADDRESS).unwrap();
    assert_eq!(
        space.map_at(ADDRESS, &allocation, 0..1, options),
        Err(MemoryError::AlreadyMapped)
    );
    assert_eq!(
        space.protect(range(ADDRESS, 2), PagePermissions::ReadWrite),
        Err(MemoryError::AccessConflict)
    );
    assert_eq!(
        space.protect(range(ADDRESS, 1), PagePermissions::ReadExecute),
        Err(MemoryError::AccessConflict)
    );
    assert_eq!(space.unmap(range(ADDRESS, 3)), Err(MemoryError::NotMapped));
    assert_eq!(space.query(ADDRESS).unwrap(), before);
    assert_eq!(allocation.info().mappings, 2);
}

fn exhaustion() {
    let manager = memory::manager();
    let space = AddressSpace::new().unwrap();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 3)
        .unwrap();
    manager.reclaim_heap();
    let held = manager
        .allocate(
            FrameOwner::Kernel,
            FramePurpose::Data,
            manager.stats().kernel.free_frames - 1,
        )
        .unwrap();
    let initial = manager.stats();
    assert_eq!(
        space.map_at(ADDRESS, &allocation, 0..3, mutable_code()),
        Err(MemoryError::NoMemory)
    );
    assert_eq!(space.query(ADDRESS).unwrap(), None);
    assert_eq!(allocation.info().mappings, 0);
    assert_eq!(manager.stats(), initial);
    drop(held);
    let mapped = space
        .map_at(ADDRESS, &allocation, 0..3, mutable_code())
        .unwrap();
    space.unmap(mapped).unwrap();
}
