// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Fallible C allocations with a retained Rust allocation layout.

use alloc::alloc::{alloc, alloc_zeroed, dealloc};
use core::{alloc::Layout, ffi::c_void, ptr};

const ALIGNMENT: usize = 16;

fn allocate(size: usize, zero: bool) -> *mut c_void {
    let Some(total) = size.max(1).checked_add(ALIGNMENT) else {
        return ptr::null_mut();
    };
    let Ok(layout) = Layout::from_size_align(total, ALIGNMENT) else {
        return ptr::null_mut();
    };
    // SAFETY: layout is nonempty and valid; failure is returned as null.
    let base = unsafe {
        if zero {
            alloc_zeroed(layout)
        } else {
            alloc(layout)
        }
    };
    if base.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: The fresh allocation contains an aligned header followed by size bytes.
    unsafe {
        base.cast::<usize>().write(total);
        base.add(ALIGNMENT).cast()
    }
}

#[unsafe(no_mangle)]
extern "C" fn arona_lwip_alloc(size: usize) -> *mut c_void {
    allocate(size, false)
}

#[unsafe(no_mangle)]
extern "C" fn arona_lwip_calloc(count: usize, size: usize) -> *mut c_void {
    count
        .checked_mul(size)
        .map_or(ptr::null_mut(), |size| allocate(size, true))
}

/// # Safety
///
/// pointer must be null or a live allocation from this adapter, freed once with
/// no remaining references or concurrent accesses.
#[unsafe(no_mangle)]
unsafe extern "C" fn arona_lwip_free(pointer: *mut c_void) {
    if pointer.is_null() {
        return;
    }
    // SAFETY: The caller transfers the live allocation including its retained header.
    unsafe {
        let base = pointer.cast::<u8>().sub(ALIGNMENT);
        let total = base.cast::<usize>().read();
        let layout = Layout::from_size_align_unchecked(total, ALIGNMENT);
        dealloc(base, layout);
    }
}

pub(super) fn new<T>(value: T) -> *mut T {
    let layout = Layout::new::<T>();
    assert!(layout.size() > 0);
    // SAFETY: Only nonempty Semaphore and Mailbox values use this helper.
    let pointer = unsafe { alloc(layout) }.cast::<T>();
    if !pointer.is_null() {
        // SAFETY: The allocation is fresh, aligned, and exclusive.
        unsafe { pointer.write(value) };
    }
    pointer
}

/// # Safety
///
/// pointer must come from new::<T>, still be live, and have no remaining users.
pub(super) unsafe fn delete<T>(pointer: *mut T) {
    // SAFETY: The caller transfers the unique allocation and its initialized value.
    unsafe {
        pointer.drop_in_place();
        dealloc(pointer.cast(), Layout::new::<T>());
    }
}
