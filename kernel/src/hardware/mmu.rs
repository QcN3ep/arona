// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Hardware capability for page table entry access, address translation switching, and cache synchronization.

use core::ops::Range;

use super::{BindError, Slot};
use crate::memory::Frame;

pub const PAGE_SIZE: usize = 4096;

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
    ExecuteOnly,
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

/// Page table levels start at 0 for the leaf level; physical read and write offsets are in units of u64.
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

    fn zero_frame(&self, physical_address: u64) -> Result<(), MmuError>;

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
