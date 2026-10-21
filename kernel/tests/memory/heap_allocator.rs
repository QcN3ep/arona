// SPDX-License-Identifier: MPL-2.0

use core::alloc::Layout;

use super::*;
use kernel::memory::HeapAllocation;

fn initialize_heap() -> &'static MemoryManager {
    let manager = initialize(&memory(8));
    manager.initialize_kernel_address_space().unwrap();
    manager.initialize_direct_map().unwrap();
    manager.kernel_address_space().activate().unwrap();
    manager
}

fn write(allocation: &HeapAllocation, value: usize) {
    let page = allocation.address() & !(PAGE_SIZE - 1);
    let offset = allocation.address() % PAGE_SIZE / size_of::<u64>();
    MMU.write_mapped_frame(page, offset, &[value as u64])
        .unwrap();
}

fn read(allocation: &HeapAllocation) -> u64 {
    let page = allocation.address() & !(PAGE_SIZE - 1);
    let offset = allocation.address() % PAGE_SIZE / size_of::<u64>();
    let mut words = [0];
    MMU.read_mapped_frame(page, offset, &mut words).unwrap();
    words[0]
}

#[test]
fn large_and_overaligned_allocations_own_writable_kernel_mappings() {
    if isolated("heap_allocator::large_and_overaligned_allocations_own_writable_kernel_mappings") {
        return;
    }
    let manager = initialize_heap();
    let initial = manager.stats();
    for (size, alignment) in [(PAGE_SIZE + 17, PAGE_SIZE * 4), (7, PAGE_SIZE * 16)] {
        let layout = Layout::from_size_align(size, alignment).unwrap();
        let allocation = manager.allocate_heap(layout).unwrap();
        assert!(allocation.address().is_multiple_of(alignment));
        assert_eq!(allocation.layout(), layout);
        assert_eq!(allocation.capacity(), size.div_ceil(PAGE_SIZE) * PAGE_SIZE);
        let stats = manager.heap_stats();
        assert_eq!(stats.allocations, 1);
        assert_eq!(stats.requested_bytes, size);
        assert_eq!(stats.allocated_bytes, allocation.capacity());
        assert_eq!(stats.backing_frames, allocation.capacity() / PAGE_SIZE);
        assert_eq!(stats.metadata_frames, 0);
        let start = allocation.address();
        for offset in (0..allocation.capacity()).step_by(PAGE_SIZE) {
            let mapping = manager
                .kernel_address_space()
                .query(start + offset)
                .unwrap()
                .unwrap()
                .mapping
                .unwrap();
            assert_eq!(mapping.attributes.permissions, PagePermissions::ReadWrite);
            assert!(!mapping.attributes.user_accessible);
            MMU.write_mapped_frame(start + offset, 0, &[offset as u64 + 1])
                .unwrap();
        }
        for offset in (0..allocation.capacity()).step_by(PAGE_SIZE) {
            let mut word = [0];
            MMU.read_mapped_frame(start + offset, 0, &mut word).unwrap();
            assert_eq!(word, [offset as u64 + 1]);
        }
        drop(allocation);
        assert_eq!(manager.kernel_address_space().query(start), Ok(None));
        assert_eq!(manager.heap_stats(), Default::default());
        assert_eq!(manager.stats(), initial);
    }
    let empty = manager
        .allocate_heap(Layout::from_size_align(0, PAGE_SIZE * 16).unwrap())
        .unwrap();
    assert!(empty.address().is_multiple_of(PAGE_SIZE * 16));
    assert_eq!(empty.capacity(), 0);
    drop(empty);
    assert_eq!(manager.heap_stats(), Default::default());
    assert_eq!(manager.stats(), initial);
}

#[test]
fn failed_heap_allocations_release_backing_and_do_not_publish_blocks() {
    if isolated("heap_allocator::failed_heap_allocations_release_backing_and_do_not_publish_blocks")
    {
        return;
    }
    let manager = initialize(&memory(8));
    let initial = manager.stats();
    assert!(matches!(
        manager.allocate_heap(Layout::new::<u64>()),
        Err(MemoryError::NotInitialized)
    ));
    assert_eq!(manager.stats(), initial);
    manager.initialize_kernel_address_space().unwrap();
    manager.initialize_direct_map().unwrap();
    assert!(matches!(
        manager.allocate_heap(Layout::new::<u64>()),
        Err(MemoryError::NotInitialized)
    ));
    manager.kernel_address_space().activate().unwrap();
    let initial = manager.stats();
    MMU.zeros_until_failure.store(1, Ordering::Relaxed);
    assert!(matches!(
        manager.allocate_heap(Layout::new::<u64>()),
        Err(MemoryError::Mmu(MmuError::InvalidTable))
    ));
    assert_eq!(manager.heap_stats(), Default::default());
    assert_eq!(manager.stats(), initial);
    MMU.writes_until_failure.store(0, Ordering::Relaxed);
    assert!(matches!(
        manager.allocate_heap(Layout::new::<[u8; PAGE_SIZE * 2]>()),
        Err(MemoryError::Mmu(MmuError::InvalidTable))
    ));
    assert_eq!(manager.heap_stats(), Default::default());
    assert_eq!(manager.stats(), initial);
    let allocation = manager.allocate_heap(Layout::new::<u64>()).unwrap();
    write(&allocation, 91);
    assert_eq!(read(&allocation), 91);
    drop(allocation);
    manager.reclaim_heap();
    assert_eq!(manager.stats(), initial);
}

#[test]
fn slots_share_frames_without_corrupting_live_allocations() {
    if isolated("heap_allocator::slots_share_frames_without_corrupting_live_allocations") {
        return;
    }
    let manager = initialize_heap();
    let initial = manager.stats();
    let layout = Layout::from_size_align(16, 16).unwrap();
    let mut allocations: Vec<_> = (0..257)
        .map(|index| {
            let allocation = manager.allocate_heap(layout).unwrap();
            write(&allocation, index);
            Some(allocation)
        })
        .collect();
    let stats = manager.heap_stats();
    assert_eq!(stats.allocations, 257);
    assert_eq!(stats.backing_frames, 2);
    assert_eq!(stats.requested_bytes, 257 * 16);
    assert_eq!(stats.allocated_bytes, 257 * 16);
    assert_eq!(
        initial.kernel.free_frames - manager.stats().kernel.free_frames,
        3
    );
    let addresses: BTreeSet<_> = allocations
        .iter()
        .map(|allocation| allocation.as_ref().unwrap().address())
        .collect();
    assert_eq!(addresses.len(), allocations.len());
    let freed = allocations[17].take().unwrap().address();
    let replacement = manager.allocate_heap(layout).unwrap();
    assert_eq!(replacement.address(), freed);
    write(&replacement, 0xfeed);
    for (index, allocation) in allocations.iter().enumerate() {
        if let Some(allocation) = allocation {
            assert_eq!(read(allocation), index as u64);
        }
    }
    drop(replacement);
    drop(allocations);
    assert_eq!(manager.heap_stats().allocations, 0);
    assert_eq!(manager.heap_stats().backing_frames, 1);
    assert_eq!(manager.heap_stats().cached_frames, 1);
    assert_eq!(manager.reclaim_heap(), 2);
    assert_eq!(manager.heap_stats(), Default::default());
    assert_eq!(manager.stats(), initial);

    let layouts: Vec<_> = (0..192)
        .map(|index: usize| {
            let alignment = 16 << index.min(7);
            Layout::from_size_align(alignment - 3, alignment).unwrap()
        })
        .collect();
    let mut allocations: Vec<_> = layouts
        .iter()
        .enumerate()
        .map(|(index, &layout)| {
            let allocation = manager.allocate_heap(layout).unwrap();
            assert_eq!(allocation.layout(), layout);
            assert!(allocation.address().is_multiple_of(layout.align()));
            assert!(allocation.capacity() >= layout.size());
            write(&allocation, index);
            Some(allocation)
        })
        .collect();
    assert!(manager.heap_stats().metadata_frames > 1);
    for index in (0..allocations.len()).step_by(3) {
        drop(allocations[index].take());
    }
    for index in (0..allocations.len()).step_by(3) {
        let allocation = manager.allocate_heap(layouts[index]).unwrap();
        write(&allocation, index);
        allocations[index] = Some(allocation);
    }
    for (index, allocation) in allocations.iter().enumerate() {
        assert_eq!(read(allocation.as_ref().unwrap()), index as u64);
    }
    drop(allocations);
    assert_eq!(manager.heap_stats().cached_frames, 8);
    manager.reclaim_heap();
    assert_eq!(manager.heap_stats(), Default::default());
    assert_eq!(manager.stats(), initial);
}

#[test]
fn pressure_reclaims_cached_slabs_and_preserves_live_objects_and_user_frames() {
    if isolated(
        "heap_allocator::pressure_reclaims_cached_slabs_and_preserves_live_objects_and_user_frames",
    ) {
        return;
    }
    let manager = initialize_heap();
    let initial = manager.stats();
    let live = manager.allocate_heap(Layout::new::<[u64; 8]>()).unwrap();
    write(&live, 73);
    drop(manager.allocate_heap(Layout::new::<u64>()).unwrap());
    assert_eq!(manager.heap_stats().cached_frames, 1);
    let held = manager
        .allocate(
            FrameOwner::Kernel,
            FramePurpose::Data,
            manager.stats().kernel.free_frames,
        )
        .unwrap();
    let replacement = manager.allocate_heap(Layout::new::<[u64; 4]>()).unwrap();
    assert_eq!(manager.heap_stats().cached_frames, 0);
    assert_eq!(read(&live), 73);
    assert_eq!(manager.stats().user, initial.user);
    let before = manager.heap_stats();
    assert!(matches!(
        manager.allocate_heap(Layout::new::<[u64; 16]>()),
        Err(MemoryError::NoMemory)
    ));
    assert_eq!(manager.heap_stats(), before);
    drop(held);
    drop(replacement);
    drop(live);
    manager.reclaim_heap();
    assert_eq!(manager.heap_stats(), Default::default());
    assert_eq!(manager.stats(), initial);
}
