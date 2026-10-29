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
        if let Err(error) = manager()
            .kernel_address_space()
            .unmap_device(self.pages, self.generation)
        {
            log::error!("Failed to release device mapping: {error:?}");
            crate::hardware::cpu().halt();
        }
    }
}

pub(crate) struct UserDeviceMapping {
    space: super::HeapArc<super::AddressSpace>,
    pages: VirtRange,
    generation: u64,
    physical: PhysRange,
}

impl UserDeviceMapping {
    pub(crate) fn new(
        space: super::HeapArc<super::AddressSpace>,
        physical: PhysRange,
    ) -> Result<Self, MemoryError> {
        if manager().direct_map.overlaps_ram(physical) {
            return Err(MemoryError::AccessConflict);
        }
        let (pages, generation) = space.map_device(physical)?;
        Ok(Self {
            space,
            pages,
            generation,
            physical,
        })
    }

    pub(crate) fn belongs_to(&self, space: &super::HeapArc<super::AddressSpace>) -> bool {
        super::HeapArc::ptr_eq(&self.space, space)
    }

    pub(crate) fn contains(&self, physical: PhysRange) -> bool {
        self.physical.start <= physical.start && physical.end <= self.physical.end
    }

    pub(crate) fn range(&self, physical: PhysRange) -> VirtRange {
        let start = self.pages.start + (physical.start - self.physical.start) as usize;
        VirtRange {
            start,
            end: start + (physical.end - physical.start) as usize,
        }
    }

    pub(crate) fn revoke(&self) -> Result<(), MemoryError> {
        self.space.revoke_device(self.pages, self.generation)
    }
}

impl Drop for UserDeviceMapping {
    fn drop(&mut self) {
        if let Err(error) = self.space.unmap_device(self.pages, self.generation) {
            log::error!("Failed to release user device mapping: {error:?}");
            crate::hardware::cpu().halt();
        }
    }
}
