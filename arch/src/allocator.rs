// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Route Rust global allocation requests to the kernel heap.

use core::alloc::{GlobalAlloc, Layout};
use core::ptr;

use kernel::memory::{self, HeapAllocation};

#[global_allocator]
static ALLOCATOR: KernelAllocator = KernelAllocator;

struct KernelAllocator;

// SAFETY: The allocation handle is retained with the storage until deallocation; the returned region is exclusive and satisfies the requested size and alignment.
unsafe impl GlobalAlloc for KernelAllocator {
    /// # Safety
    ///
    /// The MemoryManager execution environment requirements must be met during the call.
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let Ok((storage, offset)) = layout.extend(Layout::new::<HeapAllocation>()) else {
            return ptr::null_mut();
        };
        let Some(manager) = memory::try_manager() else {
            return ptr::null_mut();
        };
        let Ok(allocation) = manager.allocate_heap(storage) else {
            return ptr::null_mut();
        };
        let pointer = allocation.address() as *mut u8;
        // SAFETY: The storage must be exclusively writable; offset must lie after the payload and satisfy HeapAllocation's alignment.
        unsafe {
            pointer
                .add(offset)
                .cast::<HeapAllocation>()
                .write(allocation)
        };
        pointer
    }

    /// # Safety
    ///
    /// The GlobalAlloc deallocation contract and the MemoryManager execution environment requirements must be met.
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        let (_, offset) = layout
            .extend(Layout::new::<HeapAllocation>())
            .expect("invalid heap allocation layout");
        // SAFETY: The object reference must have been invalidated; pointer and layout must belong to one unreleased allocation of this allocator.
        let allocation = unsafe { pointer.add(offset).cast::<HeapAllocation>().read() };
        drop(allocation);
    }
}
