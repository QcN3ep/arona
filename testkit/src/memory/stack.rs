// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Measure kernel stack allocation, mapping, and release.

use kernel::{
    hardware::mmu::PAGE_SIZE,
    memory::{self, KernelStack},
};

use crate::perf::Perf;

/// Create and release a four-page kernel stack with its guard page per iteration.
pub fn perf_create_drop(perf: &mut Perf) {
    let manager = memory::manager();
    manager.reclaim_heap();
    let initial = manager.stats();
    let stack = KernelStack::new(4).unwrap();
    let range = stack.range();
    assert_eq!(range.end - range.start, 4 * PAGE_SIZE);
    drop(stack);
    let warmed = manager.stats();
    let heap = manager.heap_stats();
    perf.iter(|| KernelStack::new(4).unwrap());
    assert_eq!(manager.stats(), warmed);
    assert_eq!(manager.heap_stats(), heap);
    assert_eq!(
        manager.kernel_address_space().query(range.start).unwrap(),
        None
    );
    assert_eq!(
        manager
            .kernel_address_space()
            .query(range.start - PAGE_SIZE)
            .unwrap(),
        None
    );
    manager.reclaim_heap();
    assert_eq!(manager.stats(), initial);
}
