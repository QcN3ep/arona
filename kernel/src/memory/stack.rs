// SPDX-License-Identifier: MPL-2.0

//! Kernel stack with an unmapped guard page.

use super::{FrameOwner, FramePurpose, MemoryError, VirtRange, manager, mapping::OwnedMapping};

#[must_use]
pub struct KernelStack {
    guard: VirtRange,
    mapping: OwnedMapping,
}

impl KernelStack {
    pub fn new(frames: u64) -> Result<Self, MemoryError> {
        let backing = manager().allocate(FrameOwner::Kernel, FramePurpose::Stack, frames)?;
        let (guard, mapping) = manager().kernel_address_space().map_stack(&backing)?;
        Ok(Self { guard, mapping })
    }

    pub fn range(&self) -> VirtRange {
        self.mapping.range
    }
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        manager()
            .kernel_address_space()
            .unmap_stack(self.guard, &self.mapping)
            .expect("failed to unmap kernel stack");
    }
}
