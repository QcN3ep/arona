// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Hardware capability for page table entry access, address translation switching, and cache synchronization.

use core::ops::Range;
use core::sync::atomic::AtomicU64;

use super::{BindError, Slot};
use crate::memory::Frame;

pub const PAGE_SIZE: usize = 4096;
pub const MAX_PAGE_TABLE_LEVELS: usize = 4;
pub const PAGE_WORDS: usize = PAGE_SIZE / size_of::<u64>();

static SLOT: Slot<&'static dyn Mmu> = Slot::new();

pub fn bind(mmu: &'static dyn Mmu) -> Result<(), BindError> {
    SLOT.bind(mmu)
}

pub fn mmu() -> &'static dyn Mmu {
    SLOT.get()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MmuError {
    InvalidAddress,
    InvalidAlignment,
    InvalidRange,
    InvalidAttributes,
    InvalidTable,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PagePermissions {
    NoAccess,
    ReadOnly,
    ReadWrite,
    ReadExecute,
    ReadWriteExecute,
    ExecuteOnly,
}

impl PagePermissions {
    pub const fn writable(self) -> bool {
        matches!(self, Self::ReadWrite | Self::ReadWriteExecute)
    }

    pub const fn executable(self) -> bool {
        matches!(
            self,
            Self::ReadExecute | Self::ReadWriteExecute | Self::ExecuteOnly
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryType {
    Normal,
    Device,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageAttributes {
    pub permissions: PagePermissions,
    pub memory_type: MemoryType,
    pub user_accessible: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageMapping {
    pub physical_address: u64,
    pub attributes: PageAttributes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageTableEntry {
    Empty,
    Table(Frame),
    Mapping(PageMapping),
}

pub struct PageWalk {
    pub tables: [Option<Frame>; MAX_PAGE_TABLE_LEVELS],
    pub level: usize,
    pub entry: PageTableEntry,
}

/// Fixed page table branch required by arch for physical memory access; not owned by the caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TableWindow {
    pub virtual_address: usize,
    pub table: Frame,
    pub level: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageTableLayout {
    pub levels: usize,
    pub index_bits: u32,
    pub window: TableWindow,
}

/// Page table levels start at 0 for the leaf level.
/// `read_frame` and `write_frame` use offsets in `u64` words.
///
/// # Safety
///
/// The current processor must permit paging and cache maintenance instructions, local interrupts must be disabled, and the current page table must retain the physical access window.
/// Enabled page tables and their backing page frames must stay alive, memory accesses must not conflict with active Rust references, and modifications must not break mappings that the current execution depends on.
pub trait Mmu: Sync {
    fn layout(&self) -> PageTableLayout;
    fn is_valid_virtual_address(&self, address: usize) -> bool;
    fn physical_address_bits(&self) -> u32;

    /// Paging attributes for device registers.
    fn device_memory_type(&self) -> MemoryType;

    fn read_entry(
        &self,
        table: Frame,
        index: usize,
        level: usize,
    ) -> Result<PageTableEntry, MmuError>;

    /// Reads the path from the root through the first non-branch entry.
    ///
    /// # Safety
    ///
    /// The physical mapping must remain readable and map the stated physical range.
    /// Page table modification must be excluded throughout the walk.
    fn walk_mapped(
        &self,
        root: Frame,
        address: usize,
        physical_start: u64,
        mapping: Range<usize>,
    ) -> Result<PageWalk, MmuError>;

    /// Copies the upper half of the root table after checking that every entry is a branch.
    /// The destination entries remain unchanged on failure.
    fn copy_kernel_entries(&self, source: Frame, destination: Frame) -> Result<(), MmuError>;

    /// Checks whether every entry is empty, validating the first nonempty entry at the given level.
    fn table_is_empty(&self, table: Frame, level: usize) -> Result<bool, MmuError>;

    /// Commits the change as a complete entry without flushing the TLB; the original entry is kept on failure.
    fn write_entry(
        &self,
        table: Frame,
        index: usize,
        level: usize,
        entry: PageTableEntry,
    ) -> Result<(), MmuError>;

    /// Switches the root page table and synchronizes the local TLB; the original root is kept on failure.
    fn switch_root(&self, root: Frame) -> Result<(), MmuError>;

    fn read_frame(
        &self,
        physical_address: u64,
        offset: usize,
        words: &mut [u64],
    ) -> Result<(), MmuError>;

    fn write_frame(
        &self,
        physical_address: u64,
        offset: usize,
        words: &[u64],
    ) -> Result<(), MmuError>;

    /// Copy bytes from a normal-memory page retained for readable frame access.
    fn read_frame_bytes(
        &self,
        physical: u64,
        offset: usize,
        bytes: &mut [u8],
    ) -> Result<(), MmuError>;

    /// Copy bytes into a normal-memory page retained for writable frame access.
    fn write_frame_bytes(&self, physical: u64, offset: usize, bytes: &[u8])
    -> Result<(), MmuError>;

    /// # Safety
    ///
    /// Retain the readable normal-memory mapping and exclude conflicting references during access.
    fn read_mapped_bytes(
        &self,
        address: usize,
        offset: usize,
        bytes: &mut [u8],
    ) -> Result<(), MmuError>;

    /// # Safety
    ///
    /// Retain the writable normal-memory mapping and exclude conflicting references during access.
    fn write_mapped_bytes(
        &self,
        address: usize,
        offset: usize,
        bytes: &[u8],
    ) -> Result<(), MmuError>;

    fn zero_frame(&self, physical_address: u64) -> Result<(), MmuError>;

    /// # Safety
    ///
    /// The caller must exclusively own the complete, writable normal memory page.
    fn zero_mapped_frame(&self, address: usize) -> Result<(), MmuError>;

    /// Borrows the atomic words of a mapped normal memory page.
    ///
    /// # Safety
    ///
    /// The page must remain mapped and writable for the borrow. Its contents must not
    /// overlap other Rust references or concurrent non-atomic memory accesses.
    fn mapped_atomic_words(&self, address: usize) -> Result<&[AtomicU64; PAGE_WORDS], MmuError>;

    /// # Safety
    ///
    /// The full normal memory page must remain readable during access.
    fn read_mapped_frame(
        &self,
        address: usize,
        offset: usize,
        words: &mut [u64],
    ) -> Result<(), MmuError>;

    /// # Safety
    ///
    /// The full normal memory page must remain writable during access.
    fn write_mapped_frame(
        &self,
        address: usize,
        offset: usize,
        words: &[u64],
    ) -> Result<(), MmuError>;

    /// Atomically compares and exchanges one aligned 32-bit word at a byte
    /// offset within a mapped normal-memory page.
    ///
    /// Returns `Ok(None)` when the current word differs from `expected`; the
    /// full normal memory page must remain writable during the access.
    ///
    /// # Safety
    ///
    /// Retain the normal-memory mapping and exclude conflicting non-atomic
    /// accesses to the word for the duration of this operation.
    fn compare_exchange_u32(
        &self,
        address: usize,
        byte_offset: usize,
        expected: u32,
        new: u32,
    ) -> Result<Option<u32>, MmuError>;

    /// Invalidates the local TLB entry for the page, including global entries.
    fn invalidate_page(&self, address: usize) -> Result<(), MmuError>;
    /// Clears all local TLB entries, including global entries.
    fn invalidate_all(&self);
    /// # Safety
    ///
    /// Normal memory mappings must remain valid during synchronization, and the range must not be written.
    fn synchronize_code(&self, range: Range<usize>) -> Result<(), MmuError>;
    /// Synchronizes prior writes and subsequent instruction fetches for the physical page frame, independent of the target virtual mapping.
    fn synchronize_frame(&self, frame: Frame) -> Result<(), MmuError>;
}
