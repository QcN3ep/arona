// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn allocation_failure_rolls_back_every_taken_frame() {
    if isolated("frame_allocator::allocation_failure_rolls_back_every_taken_frame") {
        return;
    }
    let mmu = &MMU;
    let manager = initialize(&memory(4));
    let initial = manager.stats();
    mmu.zeros_until_failure.store(1, Ordering::Relaxed);
    assert!(matches!(
        manager.allocate(FrameOwner::User(1), FramePurpose::Data, 3),
        Err(MemoryError::Mmu(MmuError::InvalidTable))
    ));
    assert_eq!(manager.stats(), initial);
    assert!(matches!(
        manager.allocate(FrameOwner::User(1), FramePurpose::Data, 5),
        Err(MemoryError::NoMemory)
    ));
    assert_eq!(manager.stats(), initial);
    let frame = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 4)
        .unwrap();
    assert_eq!(
        frame
            .frames()
            .map(Frame::address)
            .collect::<BTreeSet<_>>()
            .len(),
        4
    );
    assert_eq!(manager.stats().user.free_frames, 0);
    drop(frame);
    assert_eq!(manager.stats(), initial);
}

#[test]
fn kernel_and_user_pools_do_not_borrow_from_each_other() {
    if isolated("frame_allocator::kernel_and_user_pools_do_not_borrow_from_each_other") {
        return;
    }
    let manager = initialize(&memory(4));
    let initial = manager.stats();
    let kernel = manager
        .allocate(
            FrameOwner::Kernel,
            FramePurpose::PageTable,
            initial.kernel.free_frames,
        )
        .unwrap();
    assert!(matches!(
        manager.allocate(FrameOwner::Kernel, FramePurpose::Data, 1),
        Err(MemoryError::NoMemory)
    ));
    assert_eq!(manager.stats().user.free_frames, 4);
    let user = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 4)
        .unwrap();
    assert!(kernel.frames().map(Frame::address).all(|address| {
        !user
            .frames()
            .map(Frame::address)
            .any(|other| address == other)
    }));
    drop(kernel);
    assert!(matches!(
        manager.allocate(FrameOwner::User(2), FramePurpose::Data, 1),
        Err(MemoryError::NoMemory)
    ));
    drop(user);
    assert_eq!(manager.stats(), initial);
}

#[test]
fn reserved_frames_are_reclaimed_once_after_all_borrows_end_including_frame_zero() {
    if isolated(
        "frame_allocator::reserved_frames_are_reclaimed_once_after_all_borrows_end_including_frame_zero",
    ) {
        return;
    }
    let mut info = memory(0);
    let reserved = region(0, 2, MemoryRegionState::BootReserved).range;
    info.insert(
        0,
        MemoryRegion {
            range: reserved,
            state: MemoryRegionState::BootReserved,
        },
    );
    let manager = initialize(&info);
    let initial = manager.stats();
    let first = manager.borrow_reserved(reserved).unwrap();
    let second = manager
        .borrow_reserved(PhysRange {
            start: PAGE,
            end: 2 * PAGE,
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
    assert_eq!(manager.stats(), initial);
    drop(second);
    manager.reclaim_reserved(reserved, FramePool::User).unwrap();
    let reclaimed = manager.stats();
    assert_eq!(reclaimed.reserved_frames, 0);
    assert_eq!(reclaimed.user.total_frames, 2);
    assert_eq!(
        manager.reclaim_reserved(reserved, FramePool::Kernel),
        Err(MemoryError::InvalidFrame)
    );
    assert_eq!(manager.stats(), reclaimed);
    let frame = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 2)
        .unwrap();
    assert_eq!(
        frame.frames().map(Frame::address).collect::<BTreeSet<_>>(),
        BTreeSet::from([0, PAGE])
    );
    drop(frame);
    assert_eq!(manager.stats(), reclaimed);
}

#[test]
fn reserved_range_failures_are_atomic_and_unmanaged_frames_stay_untouched() {
    if isolated(
        "frame_allocator::reserved_range_failures_are_atomic_and_unmanaged_frames_stay_untouched",
    ) {
        return;
    }
    let info = with_metadata(vec![
        region(BASE - 3 * PAGE, 1, MemoryRegionState::Reserved),
        region(BASE - 2 * PAGE, 1, MemoryRegionState::NoMap),
        region(BASE - PAGE, 1, MemoryRegionState::BootReserved),
        region(BASE, KERNEL_FRAMES, MemoryRegionState::Available),
    ]);
    let mmu = &MMU;
    let manager = initialize(&info);
    let initial = manager.stats();
    let mixed = PhysRange {
        start: BASE - PAGE,
        end: BASE + PAGE,
    };
    assert!(matches!(
        manager.borrow_reserved(mixed),
        Err(MemoryError::InvalidFrame)
    ));
    assert_eq!(
        manager.reclaim_reserved(mixed, FramePool::User),
        Err(MemoryError::InvalidFrame)
    );
    for start in [BASE - 3 * PAGE, BASE - 2 * PAGE, BASE + PAGE] {
        assert!(matches!(
            manager.borrow_reserved(PhysRange {
                start,
                end: start + PAGE
            }),
            Err(MemoryError::InvalidFrame)
        ));
        assert_eq!(
            manager.reclaim_reserved(
                PhysRange {
                    start,
                    end: start + PAGE
                },
                FramePool::User
            ),
            Err(MemoryError::InvalidFrame)
        );
    }
    assert_eq!(manager.stats(), initial);
    manager
        .reclaim_reserved(
            PhysRange {
                start: BASE - PAGE,
                end: BASE,
            },
            FramePool::Kernel,
        )
        .unwrap();
    assert_eq!(manager.stats().kernel.total_frames, KERNEL_FRAMES + 1);
    let physical = mmu.memory.lock().unwrap();
    assert!(!physical.contains_key(&(BASE - 3 * PAGE)));
    assert!(!physical.contains_key(&(BASE - 2 * PAGE)));
    assert!(!physical.contains_key(&(BASE - PAGE)));
}

#[test]
fn metadata_can_span_disjoint_regions_without_allocating_the_holes() {
    if isolated("frame_allocator::metadata_can_span_disjoint_regions_without_allocating_the_holes")
    {
        return;
    }
    let mmu = &MMU;
    let info = with_metadata(vec![
        region(0, 1, MemoryRegionState::Available),
        region(PAGE, 2, MemoryRegionState::BootReserved),
        region(4 * PAGE, 2, MemoryRegionState::Available),
        region(7 * PAGE, 1, MemoryRegionState::BootReserved),
        region(BASE, KERNEL_FRAMES + 1, MemoryRegionState::Available),
    ]);
    let manager = initialize(&info);
    let storage: BTreeSet<_> = mmu.memory.lock().unwrap().keys().copied().collect();
    assert_eq!(storage.len() as u64, manager.stats().metadata_frames);
    for address in [0, 4 * PAGE, 5 * PAGE, BASE] {
        assert!(storage.contains(&address));
    }
    assert!(storage.iter().all(|address| {
        info.iter().any(|region| {
            region.state == MemoryRegionState::Available
                && region.range.start <= *address
                && *address < region.range.end
        })
    }));
    for region in info {
        if region.state == MemoryRegionState::BootReserved {
            manager
                .reclaim_reserved(region.range, FramePool::User)
                .unwrap();
        }
    }
    let initial = manager.stats();
    let kernel = manager
        .allocate(
            FrameOwner::Kernel,
            FramePurpose::Data,
            initial.kernel.free_frames,
        )
        .unwrap();
    let user = manager
        .allocate(
            FrameOwner::User(1),
            FramePurpose::Data,
            initial.user.free_frames,
        )
        .unwrap();
    assert_eq!(initial.user.free_frames, 7);
    let allocated: BTreeSet<_> = kernel
        .frames()
        .map(Frame::address)
        .chain(user.frames().map(Frame::address))
        .collect();
    assert_eq!(
        allocated.len() as u64,
        initial.kernel.free_frames + initial.user.free_frames
    );
    assert!(allocated.is_disjoint(&storage));
    assert!(!allocated.contains(&(3 * PAGE)));
    assert!(!allocated.contains(&(6 * PAGE)));
    drop(kernel);
    drop(user);
    assert_eq!(manager.stats(), initial);
}

#[test]
fn invalid_requests_and_insufficient_addressable_ram_do_not_change_the_ledger() {
    if isolated(
        "frame_allocator::invalid_requests_and_insufficient_addressable_ram_do_not_change_the_ledger",
    ) {
        return;
    }
    for regions in [
        [region(
            BASE,
            KERNEL_FRAMES - 1,
            MemoryRegionState::Available,
        )],
        [region(
            (1 << 56) - PAGE,
            KERNEL_FRAMES + 4,
            MemoryRegionState::Available,
        )],
    ] {
        assert_eq!(memory::initialize(&regions), Err(MemoryError::NoMemory));
        assert!(MMU.memory.lock().unwrap().is_empty());
    }
    let manager = initialize(&memory(4));
    let initial = manager.stats();
    for frames in [0, u64::MAX] {
        assert!(matches!(
            manager.allocate(FrameOwner::Kernel, FramePurpose::Data, frames),
            Err(MemoryError::InvalidRequest)
        ));
    }
    assert!(matches!(
        manager.allocate(FrameOwner::User(1), FramePurpose::PageTable, 1),
        Err(MemoryError::InvalidRequest)
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

#[test]
fn invalid_region_lists_are_rejected_before_writing_ram() {
    if isolated("frame_allocator::invalid_region_lists_are_rejected_before_writing_ram") {
        return;
    }
    let valid = region(BASE, KERNEL_FRAMES, MemoryRegionState::Available);
    let later = region(valid.range.end, 1, MemoryRegionState::BootReserved);
    let cases = [
        (later.range.start, later.range.start),
        (later.range.end, later.range.start),
        (later.range.start + 1, later.range.end),
        (later.range.start, later.range.end + 1),
        (valid.range.start, valid.range.end),
        (valid.range.end - PAGE, later.range.end),
    ];
    let regions = cases
        .into_iter()
        .map(|(start, end)| {
            [
                valid,
                MemoryRegion {
                    range: PhysRange { start, end },
                    ..later
                },
            ]
        })
        .chain(core::iter::once([later, valid]));
    for regions in regions {
        assert!(matches!(
            memory::initialize(&regions),
            Err(MemoryError::InvalidRequest)
        ));
        assert!(MMU.memory.lock().unwrap().is_empty());
    }
}

#[test]
fn region_directory_respects_capacity_and_fits_a_small_stack() {
    if isolated("frame_allocator::region_directory_respects_capacity_and_fits_a_small_stack") {
        return;
    }
    let regions: Vec<_> = (0..MAX_MEMORY_REGIONS)
        .map(|index| region(index as u64 * 2 * PAGE, 1, MemoryRegionState::Available))
        .chain(core::iter::once(region(
            BASE,
            KERNEL_FRAMES,
            MemoryRegionState::Available,
        )))
        .collect();
    assert!(matches!(
        memory::initialize(&regions),
        Err(MemoryError::InvalidRequest)
    ));
    assert!(MMU.memory.lock().unwrap().is_empty());
    std::thread::Builder::new()
        .stack_size(64 * 1024)
        .spawn(|| {
            let mut info = Vec::new();
            for index in 0..MAX_MEMORY_REGIONS - 1 {
                info.push(region(
                    index as u64 * 2 * PAGE,
                    1,
                    MemoryRegionState::Available,
                ));
            }
            info.push(region(
                BASE,
                KERNEL_FRAMES + 1,
                MemoryRegionState::Available,
            ));
            let info = with_metadata(info);
            let manager = initialize(&info);
            assert_eq!(manager.stats().kernel.total_frames, KERNEL_FRAMES);
            assert_eq!(manager.stats().user.total_frames, MAX_MEMORY_REGIONS as u64);
            let frame = manager
                .allocate(
                    FrameOwner::User(1),
                    FramePurpose::Data,
                    MAX_MEMORY_REGIONS as u64,
                )
                .unwrap();
            assert_eq!(
                frame
                    .frames()
                    .map(Frame::address)
                    .collect::<BTreeSet<_>>()
                    .len(),
                MAX_MEMORY_REGIONS
            );
            assert!(frame.frames().map(Frame::address).all(|address| {
                info.iter()
                    .any(|region| region.range.start <= address && address < region.range.end)
            }));
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn metadata_does_not_consume_the_kernel_pool_or_limit_ram_to_two_gibibytes() {
    if isolated(
        "frame_allocator::metadata_does_not_consume_the_kernel_pool_or_limit_ram_to_two_gibibytes",
    ) {
        return;
    }
    let regions = [region(
        BASE,
        2 * 1024 * 1024 * 1024 / PAGE,
        MemoryRegionState::Available,
    )];
    let manager = initialize(&regions);
    let stats = manager.stats();
    assert!(stats.metadata_frames > KERNEL_FRAMES);
    assert_eq!(stats.kernel.total_frames, KERNEL_FRAMES);
    assert_eq!(stats.kernel.free_frames, KERNEL_FRAMES);
    assert_eq!(
        stats.kernel.total_frames + stats.user.total_frames + stats.metadata_frames,
        (regions[0].range.end - regions[0].range.start) / PAGE
    );
    let allocation = manager
        .allocate(FrameOwner::Kernel, FramePurpose::Data, KERNEL_FRAMES)
        .unwrap();
    assert_eq!(manager.stats().kernel.free_frames, 0);
    drop(allocation);
    assert_eq!(manager.stats(), stats);
}
