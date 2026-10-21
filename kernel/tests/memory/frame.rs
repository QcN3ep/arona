// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn allocations_keep_ownership_and_return_zeroed_frames() {
    if isolated("frame::allocations_keep_ownership_and_return_zeroed_frames") {
        return;
    }
    let mmu = &MMU;
    let manager = initialize(&memory(16));
    let initial = manager.stats();
    assert_eq!(
        memory::initialize(&memory(1)),
        Err(MemoryError::AlreadyInitialized)
    );
    assert_eq!(memory::manager().stats(), initial);
    assert_eq!(initial.kernel.total_frames, KERNEL_FRAMES);
    assert_eq!(initial.user.free_frames, 16);
    assert_eq!(initial.kernel.free_frames, KERNEL_FRAMES);
    let frame = manager
        .allocate(FrameOwner::User(7), FramePurpose::Data, 3)
        .unwrap();
    assert_eq!(frame.info().owner, FrameOwner::User(7));
    assert_eq!(frame.info().purpose, FramePurpose::Data);
    assert_eq!(frame.info().frames, 3);
    let addresses: BTreeSet<_> = frame.frames().map(Frame::address).collect();
    assert_eq!(addresses.len(), 3);
    for address in &addresses {
        let mut frame = [u64::MAX; PAGE_SIZE / 8];
        mmu.read_frame(*address, 0, &mut frame).unwrap();
        assert_eq!(frame, [0; PAGE_SIZE / 8]);
        mmu.write_frame(*address, 0, &[0xdead_beef; PAGE_SIZE / 8])
            .unwrap();
    }
    drop(frame);
    assert_eq!(manager.stats(), initial);
    let reused = manager
        .allocate(FrameOwner::User(8), FramePurpose::Stack, 3)
        .unwrap();
    assert_eq!(reused.info().owner, FrameOwner::User(8));
    assert_eq!(
        reused.frames().map(Frame::address).collect::<BTreeSet<_>>(),
        addresses
    );
    for address in reused.frames().map(Frame::address) {
        let mut frame = [u64::MAX; PAGE_SIZE / 8];
        mmu.read_frame(address, 0, &mut frame).unwrap();
        assert_eq!(frame, [0; PAGE_SIZE / 8]);
    }
}

#[test]
fn references_keep_the_entire_allocation_alive_until_the_last_use_ends() {
    if isolated("frame::references_keep_the_entire_allocation_alive_until_the_last_use_ends") {
        return;
    }
    let manager = initialize(&memory(2));
    let frame = manager
        .allocate(FrameOwner::User(9), FramePurpose::Data, 2)
        .unwrap();
    let mapping = frame.reference(ReferenceKind::Mapping).unwrap();
    let dma = frame.reference(ReferenceKind::Dma).unwrap();
    let temporary = frame.reference(ReferenceKind::Temporary).unwrap();
    assert_eq!(
        (
            frame.info().mappings,
            frame.info().dma,
            frame.info().temporary
        ),
        (1, 1, 1)
    );
    let addresses: Vec<_> = frame.frames().map(Frame::address).collect();
    drop(frame);
    let shared = mapping.reference(ReferenceKind::Mapping).unwrap();
    assert_eq!(shared.info().mappings, 2);
    drop(mapping);
    drop(dma);
    drop(temporary);
    assert_eq!(
        shared.frames().map(Frame::address).collect::<Vec<_>>(),
        addresses
    );
    assert_eq!(manager.stats().user.free_frames, 0);
    assert!(matches!(
        manager.allocate(FrameOwner::User(1), FramePurpose::Data, 1),
        Err(MemoryError::NoMemory)
    ));
    drop(shared);
    assert_eq!(manager.stats().user.free_frames, 2);
}

#[test]
fn concurrent_frames_and_references_never_share_allocated_frames() {
    if isolated("frame::concurrent_frames_and_references_never_share_allocated_frames") {
        return;
    }
    let barrier = std::sync::Barrier::new(8);
    std::thread::scope(|threads| {
        let results: Vec<_> = (0..8)
            .map(|_| {
                threads.spawn(|| {
                    barrier.wait();
                    memory::initialize(&memory(64))
                })
            })
            .collect();
        let results: Vec<_> = results
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| **result == Err(MemoryError::AlreadyInitialized))
                .count(),
            7
        );
    });
    let manager = memory::manager();
    let initial = manager.stats();
    let live = Mutex::new(BTreeSet::new());
    std::thread::scope(|threads| {
        for id in 0..8 {
            let live = &live;
            threads.spawn(move || {
                for _ in 0..32 {
                    let frame = manager
                        .allocate(FrameOwner::User(id), FramePurpose::Data, 2)
                        .unwrap();
                    let held = frame.reference(ReferenceKind::Temporary).unwrap();
                    let addresses: Vec<_> = held.frames().map(Frame::address).collect();
                    {
                        let mut live = live.lock().unwrap();
                        for address in &addresses {
                            assert!(live.insert(*address));
                        }
                    }
                    drop(frame);
                    std::thread::yield_now();
                    {
                        let mut live = live.lock().unwrap();
                        for address in &addresses {
                            assert!(live.remove(address));
                        }
                    }
                    drop(held);
                }
            });
        }
    });
    assert_eq!(manager.stats(), initial);
}

#[test]
fn one_allocation_can_own_frames_separated_by_another_allocation() {
    if isolated("frame::one_allocation_can_own_frames_separated_by_another_allocation") {
        return;
    }
    let manager = initialize(&memory(3));
    let first = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 1)
        .unwrap();
    let middle = manager
        .allocate(FrameOwner::User(2), FramePurpose::Data, 1)
        .unwrap();
    let last = manager
        .allocate(FrameOwner::User(3), FramePurpose::Data, 1)
        .unwrap();
    let expected = BTreeSet::from([
        first.frames().map(Frame::address).next().unwrap(),
        last.frames().map(Frame::address).next().unwrap(),
    ]);
    assert!(expected.last().unwrap() - expected.first().unwrap() > PAGE);
    drop(first);
    drop(last);
    let scattered = manager
        .allocate(FrameOwner::User(4), FramePurpose::Data, 2)
        .unwrap();
    assert_eq!(
        scattered
            .frames()
            .map(Frame::address)
            .collect::<BTreeSet<_>>(),
        expected
    );
    assert!(!expected.contains(&middle.frames().map(Frame::address).next().unwrap()));
}

#[test]
fn frame_access_checks_membership_and_retains_the_entire_allocation() {
    if isolated("frame::frame_access_checks_membership_and_retains_the_entire_allocation") {
        return;
    }
    let manager = initialize(&memory(3));
    let initial = manager.stats();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 2)
        .unwrap();
    let other = manager
        .allocate(FrameOwner::User(2), FramePurpose::Data, 1)
        .unwrap();
    let frame = allocation.frames().next().unwrap();
    assert!(matches!(
        allocation.access(
            other.frames().next().unwrap(),
            ReferenceKind::Temporary,
            FrameAccess::Write
        ),
        Err(MemoryError::InvalidFrame)
    ));
    assert_eq!(allocation.info().temporary, 0);
    let writer = allocation
        .access(frame, ReferenceKind::Temporary, FrameAccess::Write)
        .unwrap();
    writer.write(0, &[42]).unwrap();
    let reader = allocation
        .access(frame, ReferenceKind::Temporary, FrameAccess::Read)
        .unwrap();
    assert!(matches!(
        reader.write(0, &[0]),
        Err(MemoryError::AccessConflict)
    ));
    let mut words = [0];
    reader.read(0, &mut words).unwrap();
    assert_eq!(words, [42]);
    assert_eq!(reader.frame(), frame);
    assert!(matches!(
        reader.read(PAGE_SIZE / 8, &mut words),
        Err(MemoryError::Mmu(MmuError::InvalidRange))
    ));
    drop(allocation);
    drop(other);
    assert_eq!(manager.stats().user.free_frames, 1);
    drop(writer);
    assert_eq!(manager.stats().user.free_frames, 1);
    drop(reader);
    assert_eq!(manager.stats(), initial);
}

#[test]
fn concurrent_writers_and_executors_never_hold_conflicting_access() {
    if isolated("frame::concurrent_writers_and_executors_never_hold_conflicting_access") {
        return;
    }
    let manager = initialize(&memory(1));
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 1)
        .unwrap();
    let frame = allocation.frames().next().unwrap();
    let writers = AtomicUsize::new(0);
    let executors = AtomicUsize::new(0);
    std::thread::scope(|threads| {
        for (kind, access) in [
            (ReferenceKind::Mapping, FrameAccess::Write),
            (ReferenceKind::Mapping, FrameAccess::Execute),
            (ReferenceKind::Dma, FrameAccess::Write),
            (ReferenceKind::Temporary, FrameAccess::Write),
        ] {
            let allocation = &allocation;
            let (writers, executors) = (&writers, &executors);
            threads.spawn(move || {
                for _ in 0..64 {
                    match allocation.access(frame, kind, access) {
                        Ok(reference) => {
                            let (active, conflicting) = if access == FrameAccess::Execute {
                                (executors, writers)
                            } else {
                                (writers, executors)
                            };
                            active.fetch_add(1, Ordering::SeqCst);
                            assert_eq!(conflicting.load(Ordering::SeqCst), 0);
                            std::thread::yield_now();
                            assert_eq!(conflicting.load(Ordering::SeqCst), 0);
                            active.fetch_sub(1, Ordering::SeqCst);
                            drop(reference);
                        }
                        Err(MemoryError::AccessConflict) => std::thread::yield_now(),
                        Err(error) => panic!("unexpected frame access failure: {error:?}"),
                    }
                }
            });
        }
    });
    assert_eq!(allocation.info().mappings, 0);
    assert_eq!(allocation.info().dma, 0);
    assert_eq!(allocation.info().temporary, 0);
}
