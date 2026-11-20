// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Byte copies through retained normal-memory mappings and the physical access window.

use super::{
    Format, Mmu, MmuError, PAGE_SIZE, PagePermissions, lock_memory, validate_mapped_frame,
};

fn check(offset: usize, length: usize) -> Result<(), MmuError> {
    if offset > PAGE_SIZE || length > PAGE_SIZE - offset {
        Err(MmuError::InvalidRange)
    } else {
        Ok(())
    }
}

pub(super) fn read_frame<F: Format>(
    format: &F,
    physical: u64,
    offset: usize,
    bytes: &mut [u8],
) -> Result<(), MmuError> {
    check(offset, bytes.len())?;
    if bytes.is_empty() {
        return Ok(());
    }
    format.with_frame(physical, PagePermissions::ReadOnly, |address| {
        // SAFETY: The access window retains readable normal memory under the memory lock. The destination is exclusively borrowed and does not alias this window.
        unsafe {
            core::ptr::copy_nonoverlapping(
                (address + offset) as *const u8,
                bytes.as_mut_ptr(),
                bytes.len(),
            )
        };
    })
}

pub(super) fn write_frame<F: Format>(
    format: &F,
    physical: u64,
    offset: usize,
    bytes: &[u8],
) -> Result<(), MmuError> {
    check(offset, bytes.len())?;
    if bytes.is_empty() {
        return Ok(());
    }
    format.with_frame(physical, PagePermissions::ReadWrite, |address| {
        // SAFETY: The access window retains writable normal memory under the memory lock. The source remains borrowed and does not alias this window.
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                (address + offset) as *mut u8,
                bytes.len(),
            )
        };
    })
}

pub(super) fn read_mapped(
    mmu: &dyn Mmu,
    address: usize,
    offset: usize,
    bytes: &mut [u8],
) -> Result<(), MmuError> {
    validate_mapped_frame(mmu, address, 0, 0)?;
    check(offset, bytes.len())?;
    if bytes.is_empty() {
        return Ok(());
    }
    let _access = lock_memory();
    // SAFETY: The caller retains readable normal memory and excludes conflicting references. The destination is an exclusive, disjoint kernel buffer.
    unsafe {
        core::ptr::copy_nonoverlapping(
            (address + offset) as *const u8,
            bytes.as_mut_ptr(),
            bytes.len(),
        )
    };
    Ok(())
}

pub(super) fn write_mapped(
    mmu: &dyn Mmu,
    address: usize,
    offset: usize,
    bytes: &[u8],
) -> Result<(), MmuError> {
    validate_mapped_frame(mmu, address, 0, 0)?;
    check(offset, bytes.len())?;
    if bytes.is_empty() {
        return Ok(());
    }
    let _access = lock_memory();
    // SAFETY: The caller retains writable normal memory and excludes conflicting references. The source is a disjoint kernel buffer.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), (address + offset) as *mut u8, bytes.len())
    };
    Ok(())
}
