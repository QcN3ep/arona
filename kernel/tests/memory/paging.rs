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

#[test]
fn writable_and_executable_aliases_conflict_across_tables_but_not_between_frames() {
    if isolated(
        "paging::writable_and_executable_aliases_conflict_across_tables_but_not_between_frames",
    ) {
        return;
    }
    let manager = initialize(&memory(2));
    let initial = manager.stats();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 2)
        .unwrap();
    let frames: Vec<_> = allocation.frames().collect();
    let writable = attributes(PagePermissions::ReadWrite);
    let executable = attributes(PagePermissions::ReadExecute);
    let execute_only = attributes(PagePermissions::ExecuteOnly);
    let first = PageTable::new().unwrap();
    let second = PageTable::new().unwrap();
    first
        .map(ADDRESS, &allocation, frames[0], writable)
        .unwrap();
    assert_eq!(
        second.map(ADDRESS, &allocation, frames[0], executable),
        Err(MemoryError::AccessConflict)
    );
    second
        .map(ADDRESS, &allocation, frames[1], executable)
        .unwrap();
    assert_eq!(
        first.map(ADDRESS + PAGE_SIZE, &allocation, frames[1], writable),
        Err(MemoryError::AccessConflict)
    );
    first
        .protect(ADDRESS, PagePermissions::ReadExecute)
        .unwrap();
    second
        .map(ADDRESS + PAGE_SIZE, &allocation, frames[0], execute_only)
        .unwrap();
    assert_eq!(
        first.protect(ADDRESS, PagePermissions::ReadWrite),
        Err(MemoryError::AccessConflict)
    );
    second.unmap(ADDRESS + PAGE_SIZE).unwrap();
    first.protect(ADDRESS, PagePermissions::ReadWrite).unwrap();
    first.protect(ADDRESS, PagePermissions::ReadOnly).unwrap();
    second
        .map(ADDRESS + PAGE_SIZE, &allocation, frames[0], executable)
        .unwrap();
    assert_eq!(
        first.protect(ADDRESS, PagePermissions::ReadWrite),
        Err(MemoryError::AccessConflict)
    );
    drop(allocation);
    drop(first);
    drop(second);
    assert_eq!(manager.stats(), initial);
}

#[test]
fn dma_and_temporary_writers_block_execution_until_released() {
    if isolated("paging::dma_and_temporary_writers_block_execution_until_released") {
        return;
    }
    let manager = initialize(&memory(1));
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 1)
        .unwrap();
    let frame = allocation.frames().next().unwrap();
    let executable = attributes(PagePermissions::ReadExecute);
    let table = PageTable::new().unwrap();
    for kind in [ReferenceKind::Dma, ReferenceKind::Temporary] {
        let writer = allocation.access(frame, kind, FrameAccess::Write).unwrap();
        assert_eq!(
            table.map(ADDRESS, &allocation, frame, executable),
            Err(MemoryError::AccessConflict)
        );
        assert_eq!(allocation.info().mappings, 0);
        drop(writer);
        let reader = allocation.access(frame, kind, FrameAccess::Read).unwrap();
        table.map(ADDRESS, &allocation, frame, executable).unwrap();
        assert!(matches!(
            allocation.access(frame, kind, FrameAccess::Write),
            Err(MemoryError::AccessConflict)
        ));
        let mut word = [0];
        reader.read(0, &mut word).unwrap();
        assert_eq!(word, [0]);
        table.unmap(ADDRESS).unwrap();
        drop(allocation.access(frame, kind, FrameAccess::Write).unwrap());
    }
}

#[test]
fn execution_publication_failures_restore_permissions_and_access_counts() {
    if isolated("paging::execution_publication_failures_restore_permissions_and_access_counts") {
        return;
    }
    let manager = initialize(&memory(1));
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 1)
        .unwrap();
    let frame = allocation.frames().next().unwrap();
    let writable = attributes(PagePermissions::ReadWrite);
    let executable = attributes(PagePermissions::ReadExecute);
    let table = PageTable::new().unwrap();
    let initial = manager.stats();
    for synchronization_failure in [true, false] {
        MMU.fail_synchronization
            .store(synchronization_failure, Ordering::Relaxed);
        if !synchronization_failure {
            MMU.writes_until_failure.store(0, Ordering::Relaxed);
        }
        assert_eq!(
            table.map(ADDRESS, &allocation, frame, executable),
            Err(MemoryError::Mmu(MmuError::InvalidTable))
        );
        assert_eq!(table.query(ADDRESS).unwrap(), None);
        assert_eq!(allocation.info().mappings, 0);
        assert_eq!(manager.stats(), initial);
        drop(
            allocation
                .access(frame, ReferenceKind::Temporary, FrameAccess::Write)
                .unwrap(),
        );
    }
    table.map(ADDRESS, &allocation, frame, writable).unwrap();
    let original = table.query(ADDRESS).unwrap();
    for synchronization_failure in [true, false] {
        MMU.fail_synchronization
            .store(synchronization_failure, Ordering::Relaxed);
        if !synchronization_failure {
            MMU.writes_until_failure.store(0, Ordering::Relaxed);
        }
        assert_eq!(
            table.protect(ADDRESS, PagePermissions::ReadExecute),
            Err(MemoryError::Mmu(MmuError::InvalidTable))
        );
        assert_eq!(table.query(ADDRESS).unwrap(), original);
        assert_eq!(allocation.info().mappings, 1);
        drop(
            allocation
                .access(frame, ReferenceKind::Temporary, FrameAccess::Write)
                .unwrap(),
        );
    }
    let synchronizations = MMU.synchronizations.load(Ordering::Relaxed);
    table
        .protect(ADDRESS, PagePermissions::ReadExecute)
        .unwrap();
    assert!(MMU.synchronizations.load(Ordering::Relaxed) > synchronizations);
    assert!(matches!(
        allocation.access(frame, ReferenceKind::Dma, FrameAccess::Write),
        Err(MemoryError::AccessConflict)
    ));
    MMU.writes_until_failure.store(0, Ordering::Relaxed);
    assert_eq!(
        table.unmap(ADDRESS),
        Err(MemoryError::Mmu(MmuError::InvalidTable))
    );
    assert!(matches!(
        allocation.access(frame, ReferenceKind::Temporary, FrameAccess::Write),
        Err(MemoryError::AccessConflict)
    ));
    table.unmap(ADDRESS).unwrap();
    drop(
        allocation
            .access(frame, ReferenceKind::Dma, FrameAccess::Write)
            .unwrap(),
    );
}
