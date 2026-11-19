// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use core::marker::PhantomData;
use core::sync::atomic::{AtomicU64, Ordering};

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
    WriteExecute,
}

impl FrameAccess {
    pub const fn writable(self) -> bool {
        matches!(self, Self::Write | Self::WriteExecute)
    }

    pub const fn executable(self) -> bool {
        matches!(self, Self::Execute | Self::WriteExecute)
    }
}

impl From<PagePermissions> for FrameAccess {
    fn from(permissions: PagePermissions) -> Self {
        match permissions {
            PagePermissions::NoAccess | PagePermissions::ReadOnly => Self::Read,
            PagePermissions::ReadWrite => Self::Write,
            PagePermissions::ReadWriteExecute => Self::WriteExecute,
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
    handle_owners: AtomicU64,
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
    address: u64,
    run_remaining: u64,
    _frame: PhantomData<&'frame u64>,
}

#[must_use]
pub struct ReservedRangeReference {
    pub(super) range: PhysRange,
}

impl FrameAllocation {
    pub(super) const fn new(head: u64) -> Self {
        Self {
            head,
            handle_owners: AtomicU64::new(0),
        }
    }

    pub(crate) fn retain_handle(&self) -> Result<(), MemoryError> {
        self.handle_owners
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                count.checked_add(1)
            })
            .map(|_| ())
            .map_err(|_| MemoryError::ReferenceOverflow)
    }

    pub(crate) fn release_handle(&self) {
        let previous = self.handle_owners.fetch_sub(1, Ordering::Relaxed);
        assert!(previous != 0, "frame handle ownership underflow");
    }

    pub(crate) fn handle_owners(&self) -> u64 {
        self.handle_owners.load(Ordering::Relaxed)
    }

    pub(super) fn merge(&mut self, other: Self) {
        manager().allocator().lock().merge(self.head, other.head);
        core::mem::forget(other);
    }

    pub(super) fn detach(&mut self, frame: Frame) -> Self {
        let (remaining, head) = manager().allocator().lock().detach(self.head, frame);
        self.head = remaining;
        Self::new(head)
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
        if access.executable()
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
        if !self.access.writable() {
            return Err(MemoryError::AccessConflict);
        }
        if let Some(address) = self.virtual_address() {
            hardware::mmu().write_mapped_frame(address, word_offset, words)?;
        } else {
            hardware::mmu().write_frame(self.frame.address(), word_offset, words)?;
        }
        self.synchronize_written_code()
    }

    /// Atomically compares and exchanges one aligned 32-bit word in this frame.
    ///
    /// The operation needs the direct map, so it reports `InvalidRequest` when
    /// the frame has no kernel virtual address.
    pub fn compare_exchange_u32(
        &self,
        byte_offset: usize,
        expected: u32,
        new: u32,
    ) -> Result<Option<u32>, MemoryError> {
        if !self.access.writable() {
            return Err(MemoryError::AccessConflict);
        }
        if !byte_offset.is_multiple_of(4) || byte_offset > PAGE_SIZE - 4 {
            return Err(MemoryError::InvalidRequest);
        }
        let address = self.virtual_address().ok_or(MemoryError::InvalidRequest)?;
        let previous = hardware::mmu().compare_exchange_u32(address, byte_offset, expected, new)?;
        if previous.is_some() {
            self.synchronize_written_code()?;
        }
        Ok(previous)
    }

    fn synchronize_written_code(&self) -> Result<(), MemoryError> {
        if manager().allocator().lock().has_executors(self.frame) {
            hardware::mmu().synchronize_frame(self.frame)?;
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
            remaining: manager().frame_count(*head),
            address: 0,
            run_remaining: 0,
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
        if self.run_remaining == 0 {
            let (address, length, next) = manager().frame_run(self.next);
            self.address = address;
            self.run_remaining = length;
            self.next = next;
        }
        let address = self.address;
        self.address += PAGE_SIZE as u64;
        self.run_remaining -= 1;
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
