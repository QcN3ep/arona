// SPDX-License-Identifier: MPL-2.0

use crate::{Rights, memory::Permissions, process};
use core::{
    alloc::{GlobalAlloc, Layout},
    ptr,
};
use dlmalloc::{Allocator, Dlmalloc};
use spin::Mutex;

const PAGE_SIZE: usize = 4096;

#[global_allocator]
static HEAP: HeapAllocator = HeapAllocator(Mutex::new(Dlmalloc::new_with_allocator(System)));

struct HeapAllocator(Mutex<Dlmalloc<System>>);
struct System;

// SAFETY: 返回区间须为独占可写映射，释放须撤销完整区间。
unsafe impl Allocator for System {
    fn alloc(&self, size: usize) -> (*mut u8, usize, u32) {
        let Some(size) = size
            .checked_add(PAGE_SIZE - 1)
            .map(|size| size & !(PAGE_SIZE - 1))
        else {
            return (ptr::null_mut(), 0, 0);
        };
        let task = process::task();
        let result = task
            .allocate((size / PAGE_SIZE) as u64, Rights::MAP | Rights::READ_WRITE)
            .and_then(|frames| {
                task.map(
                    &frames,
                    0,
                    (size / PAGE_SIZE) as u64,
                    None,
                    Permissions::ReadWrite,
                )
            });
        match result {
            Ok(mapping) => {
                let range = mapping.range().unwrap();
                core::mem::forget(mapping);
                (range.start as *mut u8, size, 0)
            }
            Err(_) => (ptr::null_mut(), 0, 0),
        }
    }

    fn remap(&self, _: *mut u8, _: usize, _: usize, _: bool) -> *mut u8 {
        ptr::null_mut()
    }

    fn free_part(&self, _: *mut u8, _: usize, _: usize) -> bool {
        false
    }
    fn can_release_part(&self, _: u32) -> bool {
        false
    }

    fn free(&self, pointer: *mut u8, size: usize) -> bool {
        process::task()
            .borrow()
            .invoke(
                abi::method::task::UNMAP,
                [pointer as u64, size as u64, 0, 0, 0],
            )
            .is_ok()
    }

    fn allocates_zeros(&self) -> bool {
        true
    }
    fn page_size(&self) -> usize {
        PAGE_SIZE
    }
}

// SAFETY: 堆状态须由锁串行访问，dlmalloc 须维护块的布局与独占性。
unsafe impl GlobalAlloc for HeapAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: GlobalAlloc 调用方须提供有效布局。
        unsafe { self.0.lock().malloc(layout.size(), layout.align()) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: GlobalAlloc 调用方须提供有效布局。
        unsafe { self.0.lock().calloc(layout.size(), layout.align()) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: pointer 须为此堆按 layout 分配且尚未释放的块。
        unsafe { self.0.lock().free(pointer, layout.size(), layout.align()) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: pointer 须为此堆按 layout 分配的存活块，size 须非零。
        unsafe {
            self.0
                .lock()
                .realloc(pointer, layout.size(), layout.align(), size)
        }
    }
}
