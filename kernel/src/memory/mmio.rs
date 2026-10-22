// SPDX-License-Identifier: MPL-2.0

use super::{MemoryError, PhysRange, VirtRange, manager};

/// # Safety
///
/// 访问及析构期间须保留共享内核映射，并满足内存管理器的执行环境要求。
#[derive(Debug)]
pub struct DeviceMapping {
    pub(super) pages: VirtRange,
    pub(super) generation: u64,
    physical: PhysRange,
}

impl DeviceMapping {
    pub(crate) fn new(physical: PhysRange) -> Result<Self, MemoryError> {
        let manager = manager();
        let space = manager
            .kernel_space
            .get()
            .ok_or(MemoryError::NotInitialized)?;
        let envelope = physical
            .page_envelope()
            .map_err(|_| MemoryError::InvalidRequest)?;
        if manager.direct_map.overlaps_ram(envelope) {
            return Err(MemoryError::AccessConflict);
        }
        let (pages, generation) = space.map_device(envelope)?;
        Ok(Self {
            pages,
            generation,
            physical,
        })
    }

    pub fn physical_range(&self) -> PhysRange {
        self.physical
    }

    pub fn virtual_range(&self) -> VirtRange {
        let offset = (self.physical.start % crate::hardware::mmu::PAGE_SIZE as u64) as usize;
        let start = self.pages.start + offset;
        VirtRange {
            start,
            end: start + (self.physical.end - self.physical.start) as usize,
        }
    }
}

impl Drop for DeviceMapping {
    fn drop(&mut self) {
        if let Err(error) = manager().kernel_address_space().unmap_device(self) {
            log::error!("Failed to release device mapping: {error:?}");
            crate::hardware::cpu().halt();
        }
    }
}
