// SPDX-License-Identifier: MPL-2.0

use super::*;
use kernel::{hardware::mmu::MemoryType, memory::PageTable};

const ADDRESS: usize = 0x1_0000;

fn attributes(permissions: PagePermissions) -> PageAttributes {
    PageAttributes {
        permissions,
        memory_type: MemoryType::Normal,
        user_accessible: true,
    }
}

#[test]
fn mapping_keeps_the_entire_allocation_alive_until_after_unmapping() {
    if isolated("paging::mapping_keeps_the_entire_allocation_alive_until_after_unmapping") {
        return;
    }
    let manager = initialize(&memory(4));
    let initial = manager.stats();
    let table = PageTable::new().unwrap();
    let tables_only = manager.stats();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 3)
        .unwrap();
    let frame = allocation.frames().nth(1).unwrap();
    let copy = frame;
    assert_eq!(copy, frame);
    assert_eq!(Frame::new(frame.address() + 1), None);
    table
        .map(
            ADDRESS,
            &allocation,
            frame,
            attributes(PagePermissions::ReadWrite),
        )
        .unwrap();
    assert_eq!(allocation.info().mappings, 1);
    assert_eq!(
        table.query(ADDRESS + 23).unwrap().unwrap().physical_address,
        frame.address()
    );
    assert!(matches!(
        table.map(
            ADDRESS,
            &allocation,
            frame,
            attributes(PagePermissions::ReadOnly)
        ),
        Err(MemoryError::AlreadyMapped)
    ));
    assert_eq!(allocation.info().mappings, 1);
    drop(allocation);
    assert_eq!(manager.stats().user.free_frames, 1);
    table.protect(ADDRESS, PagePermissions::ReadOnly).unwrap();
    assert_eq!(
        table
            .query(ADDRESS)
            .unwrap()
            .unwrap()
            .attributes
            .permissions,
        PagePermissions::ReadOnly
    );
    let invalidations = MMU.invalidations.load(Ordering::Relaxed);
    table.unmap(ADDRESS).unwrap();
    assert!(MMU.invalidations.load(Ordering::Relaxed) > invalidations);
    assert_eq!(table.query(ADDRESS).unwrap(), None);
    assert_eq!(manager.stats(), tables_only);
    drop(table);
    assert_eq!(manager.stats(), initial);
    assert_eq!(copy.address(), frame.address());
}

#[test]
fn references_authorize_only_frames_from_their_own_allocation() {
    if isolated("paging::references_authorize_only_frames_from_their_own_allocation") {
        return;
    }
    let manager = initialize(&memory(3));
    let initial = manager.stats();
    let table = PageTable::new().unwrap();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 2)
        .unwrap();
    let other = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 1)
        .unwrap();
    let frame = allocation.frames().next().unwrap();
    let unrelated = other.frames().next().unwrap();
    assert!(matches!(
        table.map(
            ADDRESS,
            &allocation,
            unrelated,
            attributes(PagePermissions::ReadOnly)
        ),
        Err(MemoryError::InvalidFrame)
    ));
    assert_eq!(allocation.info().mappings, 0);
    let reference = allocation.reference(ReferenceKind::Temporary).unwrap();
    drop(allocation);
    table
        .map_reference(
            ADDRESS,
            reference,
            frame,
            attributes(PagePermissions::ReadOnly),
        )
        .unwrap();
    drop(other);
    assert_eq!(manager.stats().user.free_frames, 1);
    table.unmap(ADDRESS).unwrap();
    assert_eq!(manager.stats().user.free_frames, 3);
    drop(table);
    assert_eq!(manager.stats(), initial);
}

#[test]
fn failures_leave_no_partial_branches_or_mapping_references() {
    if isolated("paging::failures_leave_no_partial_branches_or_mapping_references") {
        return;
    }
    let manager = initialize(&memory(2));
    let before_tables = manager.stats();
    for successful_writes in 0..MMU.layout().levels as u64 - 1 {
        MMU.writes_until_failure
            .store(successful_writes, Ordering::Relaxed);
        assert!(matches!(
            PageTable::new(),
            Err(MemoryError::Mmu(MmuError::InvalidTable))
        ));
        assert_eq!(manager.stats(), before_tables);
    }
    let table = PageTable::new().unwrap();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 2)
        .unwrap();
    let frame = allocation.frames().next().unwrap();
    let initial = manager.stats();
    for successful_writes in 0..MMU.layout().levels as u64 {
        MMU.writes_until_failure
            .store(successful_writes, Ordering::Relaxed);
        assert!(matches!(
            table.map(
                ADDRESS,
                &allocation,
                frame,
                attributes(PagePermissions::ReadOnly)
            ),
            Err(MemoryError::Mmu(MmuError::InvalidTable))
        ));
        assert_eq!(table.query(ADDRESS).unwrap(), None);
        assert_eq!(allocation.info().mappings, 0);
        assert_eq!(manager.stats(), initial);
    }
    MMU.zeros_until_failure.store(1, Ordering::Relaxed);
    assert!(matches!(
        table.map(
            ADDRESS,
            &allocation,
            frame,
            attributes(PagePermissions::ReadOnly)
        ),
        Err(MemoryError::Mmu(MmuError::InvalidTable))
    ));
    assert_eq!(table.query(ADDRESS).unwrap(), None);
    assert_eq!(manager.stats(), initial);
    table
        .map(
            ADDRESS,
            &allocation,
            frame,
            attributes(PagePermissions::ReadOnly),
        )
        .unwrap();
    table
        .map(
            ADDRESS + PAGE_SIZE,
            &allocation,
            frame,
            attributes(PagePermissions::ReadOnly),
        )
        .unwrap();
    table.unmap(ADDRESS).unwrap();
    assert_eq!(table.query(ADDRESS).unwrap(), None);
    assert_eq!(
        table
            .query(ADDRESS + PAGE_SIZE)
            .unwrap()
            .unwrap()
            .physical_address,
        frame.address()
    );
    table.unmap(ADDRESS + PAGE_SIZE).unwrap();
    assert_eq!(manager.stats(), initial);
}

#[test]
fn tables_release_their_own_backing_without_disturbing_other_tables() {
    if isolated("paging::tables_release_their_own_backing_without_disturbing_other_tables") {
        return;
    }
    let manager = initialize(&memory(1));
    let initial = manager.stats();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 1)
        .unwrap();
    let frame = allocation.frames().next().unwrap();
    let first = PageTable::new().unwrap();
    let second = PageTable::new().unwrap();
    first
        .map(
            ADDRESS,
            &allocation,
            frame,
            attributes(PagePermissions::ReadOnly),
        )
        .unwrap();
    second
        .map(
            ADDRESS,
            &allocation,
            frame,
            attributes(PagePermissions::ReadOnly),
        )
        .unwrap();
    assert_eq!(allocation.info().mappings, 2);
    drop(first);
    assert_eq!(allocation.info().mappings, 1);
    assert_eq!(
        second.query(ADDRESS).unwrap().unwrap().physical_address,
        frame.address()
    );
    drop(allocation);
    assert_eq!(manager.stats().user.free_frames, 0);
    drop(second);
    assert_eq!(manager.stats(), initial);
}
