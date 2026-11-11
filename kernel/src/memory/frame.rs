// SPDX-License-Identifier: MPL-2.0

use core::marker::PhantomData;

use super::{MemoryError, PhysRange, manager};
use crate::hardware::{
    self,
    mmu::{PAGE_SIZE, PagePermissions},
};

/// Address identifier of a physical page frame; does not guarantee that the backing memory is alive.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Frame(u64);

impl Frame {
    pub const fn new(address: u64) -> Option<Self> {
        if address.is_multiple_of(PAGE_SIZE as u64) {
            Some(Self(address))
        } else {
            None
        }
    }

    pub const fn address(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FramePool {
    Kernel,
    User,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameOwner {
    Kernel,
    User(u64),
}

impl FrameOwner {
    pub const fn pool(self) -> FramePool {
        match self {
            Self::Kernel => FramePool::Kernel,
            Self::User(_) => FramePool::User,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FramePurpose {
    Data,
    PageTable,
    Stack,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReferenceKind {
    Mapping,
    Dma,
    Temporary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameAccess {
    Read,
    Write,
    Execute,
}

impl From<PagePermissions> for FrameAccess {
    fn from(permissions: PagePermissions) -> Self {
        match permissions {
            PagePermissions::NoAccess | PagePermissions::ReadOnly => Self::Read,
            PagePermissions::ReadWrite => Self::Write,
            PagePermissions::ReadExecute | PagePermissions::ExecuteOnly => Self::Execute,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameAllocationInfo {
    pub owner: FrameOwner,
    pub purpose: FramePurpose,
    pub frames: u64,
    pub mappings: u64,
    pub dma: u64,
    pub temporary: u64,
}

#[must_use]
pub struct FrameAllocation {
    pub(super) head: u64,
}

/// The allocation is returned to the pool only after the owner and all references are released.
#[must_use]
pub struct FrameAllocationReference {
    pub(super) head: u64,
    pub(super) kind: ReferenceKind,
}

#[must_use]
/// # Safety
///
/// DMA access must be stopped and drained before the reference is released.
pub struct FrameAccessReference {
    allocation: FrameAllocationReference,
    frame: Frame,
    access: FrameAccess,
    pub(super) direct_was_writable: bool,
}

pub struct Frames<'frame> {
    next: u64,
    remaining: u64,
    _frame: PhantomData<&'frame u64>,
}

#[must_use]
pub struct ReservedRangeReference {
    pub(super) range: PhysRange,
}

impl FrameAllocation {
    pub(super) fn merge(&mut self, other: Self) {
        manager().allocator().lock().merge(self.head, other.head);
        core::mem::forget(other);
    }

    pub(super) fn detach(&mut self, frame: Frame) -> Self {
        let head = manager().allocator().lock().detach(self.head, frame);
        Self { head }
    }

    pub fn info(&self) -> FrameAllocationInfo {
        manager().info(self.head)
    }

    pub fn frames(&self) -> Frames<'_> {
        Frames::new(&self.head)
    }

    pub fn contains(&self, frame: Frame) -> bool {
        manager().contains(self.head, frame)
    }

    pub fn reference(&self, kind: ReferenceKind) -> Result<FrameAllocationReference, MemoryError> {
        manager().reference(self.head, kind)
    }

    pub fn access(
        &self,
        frame: Frame,
        kind: ReferenceKind,
        access: FrameAccess,
    ) -> Result<FrameAccessReference, MemoryError> {
        self.reference(kind)?.access(frame, access)
    }
}

impl Drop for FrameAllocation {
    fn drop(&mut self) {
        manager().release_owner(self.head);
    }
}

impl FrameAllocationReference {
    pub fn info(&self) -> FrameAllocationInfo {
        manager().info(self.head)
    }

    pub fn frames(&self) -> Frames<'_> {
        Frames::new(&self.head)
    }

    pub fn contains(&self, frame: Frame) -> bool {
        manager().contains(self.head, frame)
    }

    pub fn reference(&self, kind: ReferenceKind) -> Result<Self, MemoryError> {
        manager().reference(self.head, kind)
    }

    pub fn access(
        self,
        frame: Frame,
        access: FrameAccess,
    ) -> Result<FrameAccessReference, MemoryError> {
        let mut allocator = manager().allocator().lock();
        let direct_was_writable =
            manager().reserve_access(&mut allocator, self.head, frame, self.kind, access)?;
        if access == FrameAccess::Execute
            && let Err(error) = hardware::mmu().synchronize_frame(frame)
        {
            allocator.release_access(frame, self.kind, access);
            manager().restore_direct_access(&allocator, frame, direct_was_writable);
            return Err(error.into());
        }
        drop(allocator);
        Ok(FrameAccessReference {
            allocation: self,
            frame,
            access,
            direct_was_writable,
        })
    }
}

impl FrameAccessReference {
    pub fn frame(&self) -> Frame {
        self.frame
    }

    pub fn virtual_address(&self) -> Option<usize> {
        manager().direct_map.active_address(self.frame)
    }

    pub fn read(&self, word_offset: usize, words: &mut [u64]) -> Result<(), MemoryError> {
        if let Some(address) = self.virtual_address() {
            hardware::mmu().read_mapped_frame(address, word_offset, words)?;
        } else {
            hardware::mmu().read_frame(self.frame.address(), word_offset, words)?;
        }
        Ok(())
    }

    pub fn write(&self, word_offset: usize, words: &[u64]) -> Result<(), MemoryError> {
        if self.access != FrameAccess::Write {
            return Err(MemoryError::AccessConflict);
        }
        if let Some(address) = self.virtual_address() {
            hardware::mmu().write_mapped_frame(address, word_offset, words)?;
        } else {
            hardware::mmu().write_frame(self.frame.address(), word_offset, words)?;
        }
        Ok(())
    }
}

impl Drop for FrameAccessReference {
    fn drop(&mut self) {
        manager()
            .allocator()
            .lock()
            .release_access(self.frame, self.allocation.kind, self.access);
    }
}

impl Drop for FrameAllocationReference {
    fn drop(&mut self) {
        manager().release_reference(self.head, self.kind);
    }
}

impl<'frame> Frames<'frame> {
    fn new(head: &'frame u64) -> Self {
        Frames {
            next: *head,
            remaining: manager().info(*head).frames,
            _frame: PhantomData,
        }
    }
}

impl Iterator for Frames<'_> {
    type Item = Frame;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let (address, next) = manager().frame(self.next);
        self.next = next;
        self.remaining -= 1;
        Some(Frame::new(address).expect("unaligned allocated frame"))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = usize::try_from(self.remaining).unwrap_or(usize::MAX);
        (remaining, usize::try_from(self.remaining).ok())
    }
}

impl core::iter::FusedIterator for Frames<'_> {}

impl Drop for ReservedRangeReference {
    fn drop(&mut self) {
        manager().release_reserved_reference(self.range);
    }
}
