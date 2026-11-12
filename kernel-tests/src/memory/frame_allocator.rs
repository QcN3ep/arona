// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use kernel::memory::{FrameOwner, FramePool, FramePurpose};

use super::*;

pub(super) fn run(regions: &[MemoryRegion]) {
    reserved(regions);
    requests();
    pools(regions);
    fragmented();
    contiguous();
}

fn reserved(regions: &[MemoryRegion]) {
    let manager = memory::manager();
    let initial = manager.stats();
    let reserved = regions
        .iter()
        .find(|region| region.state == MemoryRegionState::BootReserved)
        .unwrap()
        .range;
    let first = manager.borrow_reserved(reserved).unwrap();
    let second = manager
        .borrow_reserved(PhysRange {
            start: reserved.start + PAGE,
            end: reserved.end,
        })
        .unwrap();
    assert_eq!(
        manager.reclaim_reserved(reserved, FramePool::User),
        Err(MemoryError::InUse)
    );
    drop(first);
    assert_eq!(
        manager.reclaim_reserved(reserved, FramePool::User),
        Err(MemoryError::InUse)
    );
    drop(second);
    for region in regions.iter().filter(|region| {
        matches!(
            region.state,
            MemoryRegionState::Reserved | MemoryRegionState::NoMap
        )
    }) {
        assert!(matches!(
            manager.borrow_reserved(region.range),
            Err(MemoryError::InvalidFrame)
        ));
        assert_eq!(
            manager.reclaim_reserved(region.range, FramePool::User),
            Err(MemoryError::InvalidFrame)
        );
    }
    assert_eq!(manager.stats(), initial);
    manager.reclaim_reserved(reserved, FramePool::User).unwrap();
    let reclaimed = manager.stats();
    let count = (reserved.end - reserved.start) / PAGE;
    assert_eq!(reclaimed.reserved_frames, initial.reserved_frames - count);
    assert_eq!(
        reclaimed.user.total_frames,
        initial.user.total_frames + count
    );
    assert_eq!(
        manager.reclaim_reserved(reserved, FramePool::Kernel),
        Err(MemoryError::InvalidFrame)
    );
    assert_eq!(manager.stats(), reclaimed);
}

fn requests() {
    let manager = memory::manager();
    let initial = manager.stats();
    for frames in [0, u64::MAX] {
        assert!(matches!(
            manager.allocate(FrameOwner::User(1), FramePurpose::Data, frames),
            Err(MemoryError::InvalidRequest)
        ));
    }
    assert!(matches!(
        manager.allocate(FrameOwner::User(1), FramePurpose::PageTable, 1),
        Err(MemoryError::InvalidRequest)
    ));
    assert!(matches!(
        manager.allocate(
            FrameOwner::User(1),
            FramePurpose::Data,
            initial.user.free_frames + 1
        ),
        Err(MemoryError::NoMemory)
    ));
    for range in [
        PhysRange {
            start: 1,
            end: PAGE,
        },
        PhysRange {
            start: PAGE,
            end: 0,
        },
    ] {
        assert!(matches!(
            manager.borrow_reserved(range),
            Err(MemoryError::InvalidRequest)
        ));
        assert_eq!(
            manager.reclaim_reserved(range, FramePool::User),
            Err(MemoryError::InvalidRequest)
        );
    }
    assert_eq!(manager.stats(), initial);
}

fn pools(regions: &[MemoryRegion]) {
    let manager = memory::manager();
    let initial = manager.stats();
    let user = manager
        .allocate(
            FrameOwner::User(1),
            FramePurpose::Data,
            initial.user.free_frames,
        )
        .unwrap();
    assert!(matches!(
        manager.allocate(FrameOwner::User(2), FramePurpose::Data, 1),
        Err(MemoryError::NoMemory)
    ));
    let kernel = manager
        .allocate(FrameOwner::Kernel, FramePurpose::Data, 1)
        .unwrap();
    assert!(!user.contains(first_frame(&kernel)));
    assert!(user.frames().all(|frame| {
        regions.iter().any(|region| {
            matches!(
                region.state,
                MemoryRegionState::Available | MemoryRegionState::BootReserved
            ) && region.range.start <= frame.address()
                && frame.address() < region.range.end
        })
    }));
    drop(kernel);
    assert!(matches!(
        manager.allocate(FrameOwner::User(2), FramePurpose::Data, 1),
        Err(MemoryError::NoMemory)
    ));
    drop(user);
    assert_eq!(manager.stats(), initial);
}

fn fragmented() {
    let manager = memory::manager();
    let initial = manager.stats();
    let held = manager
        .allocate(
            FrameOwner::User(1),
            FramePurpose::Data,
            initial.user.free_frames - 3,
        )
        .unwrap();
    let mut candidates: [_; 3] = core::array::from_fn(|_| {
        manager
            .allocate(FrameOwner::User(1), FramePurpose::Data, 1)
            .unwrap()
    });
    candidates.sort_unstable_by_key(|allocation| first_frame(allocation).address());
    let [first, middle, last] = candidates;
    let expected = [first_frame(&first), first_frame(&last)];
    drop((first, last));
    let scattered = manager
        .allocate(FrameOwner::User(2), FramePurpose::Data, 2)
        .unwrap();
    assert_eq!(scattered.info().frames, 2);
    assert!(scattered.frames().all(|frame| expected.contains(&frame)));
    assert!(!scattered.contains(first_frame(&middle)));
    assert!(expected[1].address() - expected[0].address() > PAGE);
    drop((scattered, middle, held));
    assert_eq!(manager.stats(), initial);
}

fn contiguous() {
    let manager = memory::manager();
    let initial = manager.stats();
    for (frames, alignment) in [
        (0, PAGE),
        (3, PAGE),
        (4, 0),
        (4, 1),
        (4, 3 * PAGE),
        (4, PAGE + 1),
    ] {
        assert!(matches!(
            manager.allocate_contiguous(FrameOwner::User(1), FramePurpose::Data, frames, alignment),
            Err(MemoryError::InvalidRequest)
        ));
    }
    assert!(matches!(
        manager.allocate_contiguous(FrameOwner::User(1), FramePurpose::PageTable, 1, PAGE),
        Err(MemoryError::InvalidRequest)
    ));
    let oversized = (initial.user.free_frames + 1).next_power_of_two();
    assert!(matches!(
        manager.allocate_contiguous(FrameOwner::User(1), FramePurpose::Data, oversized, PAGE),
        Err(MemoryError::NoMemory)
    ));
    assert_eq!(manager.stats(), initial);

    let alignment = 64 * 1024;
    let allocation = manager
        .allocate_contiguous(FrameOwner::User(1), FramePurpose::Data, 16, alignment)
        .unwrap();
    assert_eq!(allocation.info().frames, 16);
    let mut previous = None;
    let mut count = 0;
    for frame in allocation.frames() {
        match previous {
            None => assert!(frame.address().is_multiple_of(alignment)),
            Some(previous) => assert_eq!(frame.address(), previous + PAGE),
        }
        previous = Some(frame.address());
        count += 1;
    }
    assert_eq!(count, 16);
    drop(allocation);
    assert_eq!(manager.stats(), initial);

    // The preceding scenarios exhausted and released the pool, so a large aligned block also
    // requires the buddy merge path to succeed.
    let wide_alignment = 128 * 1024;
    let wide = manager
        .allocate_contiguous(FrameOwner::User(1), FramePurpose::Data, 8, wide_alignment)
        .unwrap();
    assert_eq!(wide.info().frames, 8);
    assert!(first_frame(&wide).address().is_multiple_of(wide_alignment));
    drop(wide);
    assert_eq!(manager.stats(), initial);

    let block = manager
        .allocate_contiguous(FrameOwner::User(1), FramePurpose::Data, 32, 32 * PAGE)
        .unwrap();
    assert_eq!(block.info().frames, 32);
    assert!(first_frame(&block).address().is_multiple_of(32 * PAGE));
    drop(block);
    assert_eq!(manager.stats(), initial);
}
