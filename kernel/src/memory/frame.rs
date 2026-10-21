// SPDX-License-Identifier: MPL-2.0

use core::marker::PhantomData;

use super::{MemoryError, PhysRange, manager};
use crate::hardware::mmu::PAGE_SIZE;

/// 物理页框的地址标识，不保证后备内存存活。
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

/// 所有者与全部引用释放后，整次分配才归还资源池。
#[must_use]
pub struct FrameAllocationReference {
    pub(super) head: u64,
    pub(super) kind: ReferenceKind,
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
        manager().allocator.lock().merge(self.head, other.head);
        core::mem::forget(other);
    }

    pub(super) fn detach(&mut self, frame: Frame) -> Self {
        let head = manager().allocator.lock().detach(self.head, frame);
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
