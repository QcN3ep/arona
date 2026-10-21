// SPDX-License-Identifier: MPL-2.0

//! 将 Rust 全局分配请求转交内核堆。

use core::alloc::{GlobalAlloc, Layout};
use core::ptr;

use kernel::memory::{self, HeapAllocation};

#[global_allocator]
static ALLOCATOR: KernelAllocator = KernelAllocator;

struct KernelAllocator;

// SAFETY: 分配句柄随存储保留至释放，返回区域独占且满足请求的尺寸与对齐。
unsafe impl GlobalAlloc for KernelAllocator {
    /// # Safety
    ///
    /// 调用期间须满足 MemoryManager 的执行环境要求。
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
        // SAFETY: 存储须独占可写，offset 须位于载荷之后并满足 HeapAllocation 的对齐。
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
    /// 须满足 GlobalAlloc 的释放契约及 MemoryManager 的执行环境要求。
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        let (_, offset) = layout
            .extend(Layout::new::<HeapAllocation>())
            .expect("invalid heap allocation layout");
        // SAFETY: 对象引用须已失效，pointer 与 layout 须属于本分配器的一次未释放分配。
        let allocation = unsafe { pointer.add(offset).cast::<HeapAllocation>().read() };
        drop(allocation);
    }
}
