// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Word access to allocator and mapping metadata retained by the owning subsystem.

use core::sync::atomic::Ordering;

use super::{Frame, MEMORY_MANAGER, MemoryError};
use crate::hardware;

#[inline(always)]
fn mapped_address(address: u64) -> Result<Option<usize>, MemoryError> {
    if address > isize::MAX as u64 {
        return Ok(Some(address as usize));
    }
    let frame = Frame::new(address).ok_or(MemoryError::InvalidFrame)?;
    let direct_map = &MEMORY_MANAGER.direct_map;
    let mapped = direct_map.active_address(frame);
    if mapped.is_none() && direct_map.is_active() {
        return Err(MemoryError::NotMapped);
    }
    Ok(mapped)
}

/// The owner retains writable metadata storage and serializes access with its state lock.
#[inline(always)]
pub(super) fn read(address: u64, offset: usize, words: &mut [u64]) -> Result<(), MemoryError> {
    if let Some(address) = mapped_address(address)? {
        // SAFETY: The caller retains the metadata page and serializes access to its words.
        let mapped = hardware::mmu().mapped_atomic_words(address)?;
        let source = &mapped[offset..offset + words.len()];
        for (word, source) in words.iter_mut().zip(source) {
            *word = source.load(Ordering::Relaxed);
        }
    } else {
        // Direct-map construction needs metadata before the new page table is active.
        hardware::mmu().read_frame(address, offset, words)?;
    }
    Ok(())
}

/// The owner retains writable metadata storage and serializes access with its state lock.
#[inline(always)]
pub(super) fn write(address: u64, offset: usize, words: &[u64]) -> Result<(), MemoryError> {
    if let Some(address) = mapped_address(address)? {
        // SAFETY: The caller retains writable metadata and serializes access to its words.
        let mapped = hardware::mmu().mapped_atomic_words(address)?;
        for (target, word) in mapped[offset..offset + words.len()].iter().zip(words) {
            target.store(*word, Ordering::Relaxed);
        }
    } else {
        hardware::mmu().write_frame(address, offset, words)?;
    }
    Ok(())
}
