// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use core::alloc::Layout;

use kernel::memory::{FrameOwner, FramePurpose, HeapAllocation};

use super::*;

pub(super) fn run() {
    layouts();
    live_allocations();
    pressure();
}

fn layouts() {
    let manager = memory::manager();
    manager.reclaim_heap();
    let initial = manager.stats();
    let heap = manager.heap_stats();
    for (size, alignment) in [
        (0, 16 * PAGE_SIZE),
        (8, 8),
        (24, 16),
        (2048, 2048),
        (PAGE_SIZE + 17, 4 * PAGE_SIZE),
        (8, 16 * PAGE_SIZE),
    ] {
        let layout = Layout::from_size_align(size, alignment).unwrap();
        let allocation = manager.allocate_heap(layout).unwrap();
        assert!(allocation.address().is_multiple_of(alignment));
        assert_eq!(allocation.layout(), layout);
        assert!(allocation.capacity() >= size);
        if size != 0 {
            let stats = manager.heap_stats();
            assert_eq!(stats.allocations, heap.allocations + 1);
            assert_eq!(stats.requested_bytes, heap.requested_bytes + size);
            assert_eq!(
                stats.allocated_bytes,
                heap.allocated_bytes + allocation.capacity()
            );
            for offset in (0..size.saturating_sub(7)).step_by(PAGE_SIZE) {
                let address = allocation.address() + offset;
                let page = address & !(PAGE_SIZE - 1);
                let word = address % PAGE_SIZE / size_of::<u64>();
                hardware::mmu()
                    .write_mapped_frame(page, word, &[offset as u64 + 1])
                    .unwrap();
                let mut value = [0];
                hardware::mmu()
                    .read_mapped_frame(page, word, &mut value)
                    .unwrap();
                assert_eq!(value, [offset as u64 + 1]);
            }
        }
        drop(allocation);
        manager.reclaim_heap();
        assert_eq!(manager.heap_stats(), heap);
        assert_eq!(manager.stats(), initial);
    }
}

fn live_allocations() {
    let manager = memory::manager();
    let initial = manager.stats();
    let heap = manager.heap_stats();
    let mut allocations: [Option<HeapAllocation>; 64] = core::array::from_fn(|index| {
        let alignment = 16 << (index % 8);
        let layout = Layout::from_size_align(alignment - 3, alignment).unwrap();
        let allocation = manager.allocate_heap(layout).unwrap();
        assert!(allocation.address().is_multiple_of(alignment));
        assert!(allocation.capacity() >= layout.size());
        write(&allocation, index as u64);
        Some(allocation)
    });
    for index in (0..allocations.len()).step_by(3) {
        let allocation = allocations[index].take().unwrap();
        let layout = allocation.layout();
        drop(allocation);
        let replacement = manager.allocate_heap(layout).unwrap();
        write(&replacement, index as u64);
        allocations[index] = Some(replacement);
    }
    manager.reclaim_heap();
    for (index, allocation) in allocations.iter().enumerate() {
        let allocation = allocation.as_ref().unwrap();
        assert_eq!(read(allocation), index as u64);
        for other in &allocations[..index] {
            let other = other.as_ref().unwrap();
            assert!(
                allocation.address() + allocation.capacity() <= other.address()
                    || other.address() + other.capacity() <= allocation.address()
            );
        }
    }
    assert_eq!(manager.stats().user, initial.user);
    drop(allocations);
    manager.reclaim_heap();
    assert_eq!(manager.heap_stats(), heap);
    assert_eq!(manager.stats(), initial);
}

fn pressure() {
    let manager = memory::manager();
    let initial = manager.stats();
    let heap = manager.heap_stats();
    let live = manager.allocate_heap(Layout::new::<u64>()).unwrap();
    write(&live, 73);
    let held = manager
        .allocate(
            FrameOwner::Kernel,
            FramePurpose::Data,
            manager.stats().kernel.free_frames,
        )
        .unwrap();
    let before = manager.heap_stats();
    let layout = Layout::from_size_align(8 * PAGE_SIZE, PAGE_SIZE).unwrap();
    assert!(matches!(
        manager.allocate_heap(layout),
        Err(MemoryError::NoMemory)
    ));
    assert_eq!(manager.heap_stats(), before);
    assert_eq!(read(&live), 73);
    assert_eq!(manager.stats().user, initial.user);
    drop(held);
    let recovered = manager.allocate_heap(layout).unwrap();
    write(&recovered, 91);
    assert_eq!(read(&recovered), 91);
    assert_eq!(read(&live), 73);
    drop((recovered, live));
    manager.reclaim_heap();
    assert_eq!(manager.heap_stats(), heap);
    assert_eq!(manager.stats(), initial);
}

fn write(allocation: &HeapAllocation, value: u64) {
    let page = allocation.address() & !(PAGE_SIZE - 1);
    let offset = allocation.address() % PAGE_SIZE / size_of::<u64>();
    hardware::mmu()
        .write_mapped_frame(page, offset, &[value])
        .unwrap();
}

fn read(allocation: &HeapAllocation) -> u64 {
    let page = allocation.address() & !(PAGE_SIZE - 1);
    let offset = allocation.address() % PAGE_SIZE / size_of::<u64>();
    let mut words = [0];
    hardware::mmu()
        .read_mapped_frame(page, offset, &mut words)
        .unwrap();
    words[0]
}
