// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Check address spaces and measure mapping, protection, lookup, and reservation operations.

use core::hint::black_box;

use kernel::memory::{
    FrameAccess, FrameOwner, FramePurpose, ReferenceKind, USER_ADDRESS_RANGE,
    allocated_address_spaces,
};

use super::*;
use crate::perf::Perf;

pub(super) fn run() {
    mappings();
    partial_unmap();
    mapping_conflict();
    reservations();
    shared_kernel();
    kernel_permissions();
    user_permissions();
    invalid_requests();
    exhaustion();
    huge_mappings();
}

/// Create and destroy one inactive user address space per iteration.
pub fn perf_create_drop(perf: &mut Perf) {
    let manager = memory::manager();
    manager.reclaim_heap();
    let initial = manager.stats();
    let spaces = allocated_address_spaces();
    perf.iter(|| AddressSpace::new().unwrap());
    assert_eq!(allocated_address_spaces(), spaces);
    manager.reclaim_heap();
    assert_eq!(manager.stats(), initial);
}

/// Map and unmap prepared physical pages at a fixed user address per iteration.
pub fn perf_map_unmap(perf: &mut Perf, pages: u64) {
    let manager = memory::manager();
    manager.reclaim_heap();
    let initial = manager.stats();
    let space = AddressSpace::new().unwrap();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, pages)
        .unwrap();
    let options = MappingOptions::new(PagePermissions::ReadWrite);
    let mapped = space
        .map_at(ADDRESS, &allocation, 0..pages, options)
        .unwrap();
    space.unmap(mapped).unwrap();
    let warmed = manager.stats();
    let heap = manager.heap_stats();
    let info = allocation.info();
    perf.iter(|| {
        let mapped = space
            .map_at(ADDRESS, &allocation, 0..pages, options)
            .unwrap();
        space.unmap(mapped).unwrap();
    });
    assert_eq!(space.query(ADDRESS).unwrap(), None);
    assert_eq!(space.query(mapped.end - 1).unwrap(), None);
    assert_eq!(allocation.info(), info);
    assert_eq!(manager.stats(), warmed);
    assert_eq!(manager.heap_stats(), heap);
    drop((space, allocation));
    manager.reclaim_heap();
    assert_eq!(manager.stats(), initial);
}

/// Change one mapped page from RW to `permissions` and back to RW per iteration.
pub fn perf_protect(perf: &mut Perf, permissions: PagePermissions) {
    let manager = memory::manager();
    manager.reclaim_heap();
    let initial = manager.stats();
    let space = AddressSpace::new().unwrap();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 1)
        .unwrap();
    let mapped = space
        .map_at(ADDRESS, &allocation, 0..1, mutable_code())
        .unwrap();
    let info = allocation.info();
    let before = manager.stats();
    perf.iter(|| {
        space.protect(mapped, permissions).unwrap();
        space.protect(mapped, PagePermissions::ReadWrite).unwrap();
    });
    let mapping = space.query(ADDRESS).unwrap().unwrap().mapping.unwrap();
    assert_eq!(mapping.attributes.permissions, PagePermissions::ReadWrite);
    assert_eq!(allocation.info(), info);
    assert_eq!(manager.stats(), before);
    let writer = allocation
        .access(
            first_frame(&allocation),
            ReferenceKind::Temporary,
            FrameAccess::Write,
        )
        .unwrap();
    writer.write(0, &[73]).unwrap();
    let mut value = [0];
    writer.read(0, &mut value).unwrap();
    assert_eq!(value, [73]);
    drop(writer);
    space.unmap(mapped).unwrap();
    drop((space, allocation));
    manager.reclaim_heap();
    assert_eq!(manager.stats(), initial);
}

/// Query fixed hits or holes among `entries` mappings, with four lookups per iteration.
pub fn perf_query(perf: &mut Perf, entries: usize, hit: bool) {
    let manager = memory::manager();
    manager.reclaim_heap();
    let initial = manager.stats();
    let space = AddressSpace::new().unwrap();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, entries as u64)
        .unwrap();
    for index in 0..entries {
        space
            .map_at(
                ADDRESS + 2 * index * PAGE_SIZE,
                &allocation,
                index as u64..index as u64 + 1,
                MappingOptions::new(PagePermissions::ReadOnly),
            )
            .unwrap();
    }
    let addresses = [0, entries / 3, entries / 2, entries - 1]
        .map(|index| ADDRESS + (2 * index + usize::from(!hit)) * PAGE_SIZE);
    let expected = addresses.map(|address| space.query(address).unwrap());
    assert!(expected.iter().all(|mapping| mapping.is_some() == hit));
    let before = manager.stats();
    perf.iter(|| {
        for address in black_box(addresses) {
            black_box(space.query(address).unwrap());
        }
    });
    assert_eq!(
        addresses.map(|address| space.query(address).unwrap()),
        expected
    );
    assert_eq!(manager.stats(), before);
    drop((space, allocation));
    manager.reclaim_heap();
    assert_eq!(manager.stats(), initial);
}

/// Reserve and release four pages among 16 fixed background ranges per iteration.
pub fn perf_reserve_release(perf: &mut Perf) {
    let manager = memory::manager();
    manager.reclaim_heap();
    let initial = manager.stats();
    let space = AddressSpace::new().unwrap();
    let occupied = core::array::from_fn::<_, 16, _>(|index| {
        range(USER_ADDRESS_RANGE.start + index * 8 * PAGE_SIZE, 4)
    });
    for reserved in occupied {
        space.reserve_at(reserved).unwrap();
    }
    let sample = space.reserve(4 * PAGE_SIZE, PAGE_SIZE).unwrap();
    assert!(occupied.iter().all(|&occupied| !occupied.overlaps(sample)));
    space.unreserve(sample).unwrap();
    let warmed = manager.stats();
    let heap = manager.heap_stats();
    perf.iter(|| {
        let reserved = space.reserve(4 * PAGE_SIZE, PAGE_SIZE).unwrap();
        space.unreserve(reserved).unwrap();
    });
    assert_eq!(space.query(sample.start).unwrap(), None);
    for reserved in occupied {
        let entry = space.query(reserved.start).unwrap().unwrap();
        assert!(entry.mapping.is_none());
        assert_eq!(entry.range, reserved);
    }
    assert_eq!(manager.stats(), warmed);
    assert_eq!(manager.heap_stats(), heap);
    drop(space);
    manager.reclaim_heap();
    assert_eq!(manager.stats(), initial);
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

fn partial_unmap() {
    let manager = memory::manager();
    manager.reclaim_heap();
    let initial = manager.stats();
    let space = AddressSpace::new().unwrap();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 5)
        .unwrap();
    let options = MappingOptions::new(PagePermissions::ReadWrite);
    let boundary = PAGE_SIZE << hardware::mmu().layout().index_bits;
    let start = boundary - PAGE_SIZE;
    let mapped = space.map_at(start, &allocation, 0..5, options).unwrap();
    let first = space.query(start).unwrap();
    let last = space.query(mapped.end - 1).unwrap();
    let middle = range(boundary, 3);
    space.unmap(middle).unwrap();
    assert_eq!(allocation.info().mappings, 2);
    assert_eq!(space.query(start).unwrap(), first);
    assert_eq!(space.query(mapped.end - 1).unwrap(), last);
    for address in (middle.start..middle.end).step_by(PAGE_SIZE) {
        assert_eq!(space.query(address).unwrap(), None);
    }
    assert_eq!(
        space
            .map_at(middle.start, &allocation, 1..4, options)
            .unwrap(),
        middle
    );
    assert_eq!(allocation.info().mappings, 5);
    space.unmap(mapped).unwrap();
    assert_eq!(allocation.info().mappings, 0);
    for address in (mapped.start..mapped.end).step_by(PAGE_SIZE) {
        assert_eq!(space.query(address).unwrap(), None);
    }
    drop((space, allocation));
    manager.reclaim_heap();
    assert_eq!(manager.stats(), initial);
}

fn mapping_conflict() {
    let manager = memory::manager();
    manager.reclaim_heap();
    let initial = manager.stats();
    let space = AddressSpace::new().unwrap();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 3)
        .unwrap();
    let last = allocation.frames().last().unwrap();
    let writer = allocation
        .access(last, ReferenceKind::Dma, FrameAccess::Write)
        .unwrap();
    let info = allocation.info();
    let before = manager.stats();
    assert_eq!(
        space.map_at(
            ADDRESS,
            &allocation,
            0..3,
            MappingOptions::new(PagePermissions::ReadExecute)
        ),
        Err(MemoryError::AccessConflict)
    );
    for address in (ADDRESS..ADDRESS + 3 * PAGE_SIZE).step_by(PAGE_SIZE) {
        assert_eq!(space.query(address).unwrap(), None);
    }
    assert_eq!(allocation.info(), info);
    assert_eq!(manager.stats(), before);
    for frame in allocation.frames() {
        allocation
            .access(frame, ReferenceKind::Temporary, FrameAccess::Write)
            .unwrap()
            .write(0, &[73])
            .unwrap();
    }
    drop(writer);
    let mapped = space
        .map_at(ADDRESS, &allocation, 0..3, mutable_code())
        .unwrap();
    space.unmap(mapped).unwrap();
    drop((space, allocation));
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
    reservation_reuse(&space);
    drop((space, other));
    manager.reclaim_heap();
    assert_eq!(manager.stats(), initial);
}

fn reservation_reuse(space: &AddressSpace) {
    let manager = memory::manager();
    let first = range(ADDRESS, 1);
    space.reserve_at(first).unwrap();
    let occupied = manager.stats();
    space.unreserve(first).unwrap();
    let ranges =
        core::array::from_fn::<_, 96, _>(|index| range(ADDRESS + index * 2 * PAGE_SIZE, 1));
    for reserved in ranges {
        space.reserve_at(reserved).unwrap();
    }
    for reserved in &ranges[..ranges.len() - 1] {
        space.unreserve(*reserved).unwrap();
    }
    let last = *ranges.last().unwrap();
    assert_eq!(space.query(last.start).unwrap().unwrap().range, last);
    assert_eq!(manager.stats(), occupied);
    space.unreserve(last).unwrap();
    for reserved in ranges.iter().step_by(4) {
        space.reserve_at(*reserved).unwrap();
    }
    let occupied = manager.stats();
    for reserved in ranges
        .iter()
        .enumerate()
        .filter_map(|(i, r)| (i % 4 != 0).then_some(r))
    {
        space.reserve_at(*reserved).unwrap();
    }
    for reserved in ranges
        .iter()
        .enumerate()
        .rev()
        .filter_map(|(i, r)| (i % 4 != 0).then_some(r))
    {
        space.unreserve(*reserved).unwrap();
    }
    for reserved in ranges.iter().step_by(4) {
        assert_eq!(
            space.query(reserved.start).unwrap().unwrap().range,
            *reserved
        );
    }
    assert_eq!(manager.stats(), occupied);
    for reserved in ranges.iter().step_by(4) {
        space.unreserve(*reserved).unwrap();
    }
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

fn kernel_permissions() {
    let manager = memory::manager();
    let space = manager.kernel_address_space();
    let allocation = manager
        .allocate(FrameOwner::Kernel, FramePurpose::Data, 1)
        .unwrap();
    let frame = first_frame(&allocation);
    let first = space.map(&allocation, 0..1, mutable_code()).unwrap();
    let second = space.map(&allocation, 0..1, mutable_code()).unwrap();
    let before = space.query(first.start).unwrap();
    assert_eq!(
        space.protect(first, PagePermissions::ReadExecute),
        Err(MemoryError::AccessConflict)
    );
    assert_eq!(space.query(first.start).unwrap(), before);
    space.protect(second, PagePermissions::ReadOnly).unwrap();
    space.protect(first, PagePermissions::ReadExecute).unwrap();
    assert!(matches!(
        allocation.access(frame, ReferenceKind::Dma, FrameAccess::Write),
        Err(MemoryError::AccessConflict)
    ));
    assert_eq!(
        space.protect(second, PagePermissions::ReadWrite),
        Err(MemoryError::AccessConflict)
    );
    assert_eq!(
        lookup(space, first.start).unwrap().attributes.permissions,
        PagePermissions::ReadExecute
    );
    assert_eq!(
        space.protect(first, PagePermissions::ReadWriteExecute),
        Err(MemoryError::AccessConflict)
    );
    space.unmap(first).unwrap();
    space.protect(second, PagePermissions::ReadWrite).unwrap();
    space.unmap(second).unwrap();
    assert_eq!(allocation.info().mappings, 0);
}

fn user_permissions() {
    let manager = memory::manager();
    manager.reclaim_heap();
    let initial = manager.stats();
    let space = AddressSpace::new().unwrap();
    let other = AddressSpace::new().unwrap();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 1)
        .unwrap();
    let frame = first_frame(&allocation);
    let maximum = MappingRights::READ | MappingRights::WRITE | MappingRights::EXECUTE;
    for (index, permissions) in [
        PagePermissions::ReadWrite,
        PagePermissions::ReadExecute,
        PagePermissions::ReadWriteExecute,
    ]
    .into_iter()
    .enumerate()
    {
        space
            .map_at(
                ADDRESS + index * PAGE_SIZE,
                &allocation,
                0..1,
                MappingOptions {
                    maximum,
                    ..MappingOptions::new(permissions)
                },
            )
            .unwrap();
        assert_eq!(
            lookup(&space, ADDRESS + index * PAGE_SIZE)
                .unwrap()
                .attributes
                .permissions,
            permissions
        );
    }
    let writer = allocation
        .access(frame, ReferenceKind::Temporary, FrameAccess::Write)
        .unwrap();
    writer.write(0, &[73]).unwrap();
    drop(writer);
    let before = allocation.info();
    assert!(matches!(
        allocation.access(frame, ReferenceKind::Dma, FrameAccess::Write),
        Err(MemoryError::AccessConflict)
    ));
    assert_eq!(allocation.info(), before);
    let alias = other
        .map(
            &allocation,
            0..1,
            MappingOptions::new(PagePermissions::ReadExecute),
        )
        .unwrap();
    for permissions in [
        PagePermissions::ReadOnly,
        PagePermissions::ReadWriteExecute,
        PagePermissions::ReadExecute,
        PagePermissions::ReadWrite,
    ] {
        space.protect(range(ADDRESS, 3), permissions).unwrap();
        assert_eq!(
            lookup(&space, ADDRESS).unwrap().attributes.permissions,
            permissions
        );
    }
    space.unmap(range(ADDRESS, 3)).unwrap();
    other.unmap(alias).unwrap();
    assert_eq!(allocation.info().mappings, 0);
    drop((allocation, space, other));
    manager.reclaim_heap();
    assert_eq!(manager.stats(), initial);
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

fn huge_mappings() {
    use kernel::memory::{HUGE_PAGE_FRAMES, HUGE_PAGE_SIZE};

    let manager = memory::manager();
    manager.reclaim_heap();
    let initial = manager.stats();
    let space = AddressSpace::new().unwrap();
    let allocation = manager
        .allocate_contiguous(
            FrameOwner::User(1),
            FramePurpose::Data,
            HUGE_PAGE_FRAMES,
            HUGE_PAGE_SIZE as u64,
        )
        .unwrap();
    let frames: alloc::vec::Vec<Frame> = allocation.frames().collect();
    assert_eq!(frames.len(), HUGE_PAGE_FRAMES as usize);
    let options = MappingOptions {
        permissions: PagePermissions::ReadWrite,
        maximum: MappingRights::READ | MappingRights::WRITE | MappingRights::EXECUTE,
        alignment: PAGE_SIZE,
    };
    let mapped = space
        .map_huge(&allocation, 0..HUGE_PAGE_FRAMES, options)
        .unwrap();
    assert!(mapped.start.is_multiple_of(HUGE_PAGE_SIZE));
    assert_eq!(mapped.end - mapped.start, HUGE_PAGE_SIZE);
    assert_eq!(allocation.info().mappings, HUGE_PAGE_FRAMES);
    for index in [0, 1, HUGE_PAGE_FRAMES as usize - 1] {
        assert_eq!(
            space
                .translate(mapped.start + index * PAGE_SIZE + 7)
                .unwrap(),
            Some(frames[index].address() + 7)
        );
    }
    let info = space.query(mapped.start + PAGE_SIZE).unwrap().unwrap();
    assert_eq!(info.range, mapped);
    assert_eq!(
        info.mapping.unwrap().attributes.permissions,
        PagePermissions::ReadWrite
    );
    space
        .protect(mapped, PagePermissions::ReadWriteExecute)
        .unwrap();
    assert_eq!(
        lookup(&space, mapped.end - PAGE_SIZE)
            .unwrap()
            .attributes
            .permissions,
        PagePermissions::ReadWriteExecute
    );
    space.protect(mapped, PagePermissions::ReadOnly).unwrap();
    assert_eq!(
        space
            .query(mapped.start)
            .unwrap()
            .unwrap()
            .mapping
            .unwrap()
            .attributes
            .permissions,
        PagePermissions::ReadOnly
    );
    space.protect(mapped, PagePermissions::ReadWrite).unwrap();
    assert_eq!(
        space.unmap(range(mapped.start + PAGE_SIZE, 1)),
        Err(MemoryError::InUse)
    );
    space.unmap(mapped).unwrap();
    assert_eq!(allocation.info().mappings, 0);
    assert_eq!(space.translate(mapped.start).unwrap(), None);
    drop((frames, allocation, space));
    manager.reclaim_heap();
    assert_eq!(manager.stats(), initial);
}
