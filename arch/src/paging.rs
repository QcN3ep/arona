// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use core::{
    ops::Range,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
use kernel::{
    hardware::mmu::{
        MAX_PAGE_TABLE_LEVELS, Mmu, MmuError, PAGE_SIZE, PAGE_WORDS, PageMapping, PagePermissions,
        PageTableEntry, PageTableLayout, PageWalk, TableWindow,
    },
    memory::Frame,
};

const ENTRIES: usize = PAGE_SIZE / size_of::<u64>();
pub(crate) const WINDOW_INDEX: usize = ENTRIES - 1;
pub(crate) const PAGE_TABLE_WINDOW: usize = usize::MAX & !(PAGE_SIZE - 1);

#[cfg(target_arch = "riscv64")]
const KERNEL_OFFSET: usize = 0xffff_ffc0_0000_0000 - 0x8020_0000;
#[cfg(target_arch = "loongarch64")]
const KERNEL_OFFSET: usize = 0xffff_8000_0000_0000;

pub(crate) trait Format: Mmu {
    const LEVELS: usize;
    const PHYSICAL_BITS: u32;

    fn encode_table(&self, address: u64) -> u64;
    fn decode_table(&self, entry: u64) -> Result<u64, MmuError>;
    fn encode_mapping(&self, mapping: PageMapping) -> Result<u64, MmuError>;
    fn decode_mapping(&self, entry: u64) -> Result<PageMapping, MmuError>;
    fn window_table_address(&self) -> u64 {
        window_table_physical()
    }
    fn with_table<T>(
        &self,
        address: u64,
        operation: impl FnOnce(&mut PageTable) -> T,
    ) -> Result<T, MmuError> {
        self.with_frame(address, PagePermissions::ReadWrite, |address| {
            // SAFETY: The window must exclusively map the complete writable page frame.
            operation(unsafe { &mut *(address as *mut PageTable) })
        })
    }
    fn with_frame<T>(
        &self,
        address: u64,
        permissions: PagePermissions,
        operation: impl FnOnce(usize) -> T,
    ) -> Result<T, MmuError>;
}

#[repr(C, align(4096))]
pub(crate) struct PageTable {
    entries: [u64; ENTRIES],
}

#[used]
#[unsafe(link_section = ".boot_tables.window")]
pub(crate) static mut PAGE_TABLE_WINDOW_TABLE: PageTable = PageTable::new();

static MEMORY_ACCESS_LOCK: AtomicBool = AtomicBool::new(false);

pub(crate) struct MemoryAccessGuard;

pub(crate) fn lock_memory() -> MemoryAccessGuard {
    while MEMORY_ACCESS_LOCK
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    MemoryAccessGuard
}

impl Drop for MemoryAccessGuard {
    fn drop(&mut self) {
        MEMORY_ACCESS_LOCK.store(false, Ordering::Release);
    }
}

impl PageTable {
    const fn new() -> Self {
        Self {
            entries: [0; ENTRIES],
        }
    }

    pub(crate) fn read(&self, index: usize) -> u64 {
        // SAFETY: index must be within the page table, and hardware and other processors must not modify the entry concurrently.
        unsafe { (&raw const self.entries[index]).read_volatile() }
    }

    pub(crate) fn write(&mut self, index: usize, entry: u64) {
        // SAFETY: index must be within the exclusive page table; the entry is committed as an aligned, full 64-bit store.
        unsafe { (&raw mut self.entries[index]).write_volatile(entry) }
    }
}

pub(crate) fn window_table_physical() -> u64 {
    (core::ptr::addr_of!(PAGE_TABLE_WINDOW_TABLE) as usize - KERNEL_OFFSET) as u64
}

pub(crate) fn read_frame<F: Format>(
    format: &F,
    address: u64,
    offset: usize,
    words: &mut [u64],
) -> Result<(), MmuError> {
    if offset > ENTRIES || words.len() > ENTRIES - offset {
        return Err(MmuError::InvalidRange);
    }
    format.with_frame(address, PagePermissions::ReadOnly, |address| {
        // SAFETY: The window must map the complete page frame read-only, and no write reference may conflict during access.
        let page = unsafe { &*(address as *const PageTable) };
        for (index, word) in words.iter_mut().enumerate() {
            *word = page.read(offset + index);
        }
    })
}

pub(crate) fn write_frame<F: Format>(
    format: &F,
    address: u64,
    offset: usize,
    words: &[u64],
) -> Result<(), MmuError> {
    if offset > ENTRIES || words.len() > ENTRIES - offset {
        return Err(MmuError::InvalidRange);
    }
    format.with_table(address, |page| {
        for (index, word) in words.iter().enumerate() {
            page.write(offset + index, *word);
        }
    })
}

fn validate_mapped_frame(
    mmu: &dyn Mmu,
    address: usize,
    offset: usize,
    length: usize,
) -> Result<(), MmuError> {
    if !mmu.is_valid_virtual_address(address) {
        return Err(MmuError::InvalidAddress);
    }
    if !address.is_multiple_of(PAGE_SIZE) {
        return Err(MmuError::InvalidAlignment);
    }
    if offset > ENTRIES || length > ENTRIES - offset {
        return Err(MmuError::InvalidRange);
    }
    Ok(())
}

pub(crate) fn read_mapped_frame(
    mmu: &dyn Mmu,
    address: usize,
    offset: usize,
    words: &mut [u64],
) -> Result<(), MmuError> {
    validate_mapped_frame(mmu, address, offset, words.len())?;
    let _access = lock_memory();
    let mut index = 0;
    while index < words.len() {
        let pointer = (address + (offset + index) * size_of::<u64>()) as *const u64;
        // SAFETY: The mapping must remain readable, and the access must not conflict with an active write reference.
        words[index] = unsafe { pointer.read_volatile() };
        index += 1;
    }
    Ok(())
}

pub(crate) fn write_mapped_frame(
    mmu: &dyn Mmu,
    address: usize,
    offset: usize,
    words: &[u64],
) -> Result<(), MmuError> {
    validate_mapped_frame(mmu, address, offset, words.len())?;
    let _access = lock_memory();
    let mut index = 0;
    while index < words.len() {
        let pointer = (address + (offset + index) * size_of::<u64>()) as *mut u64;
        // SAFETY: The mapping must remain writable, and the access must not conflict with an active Rust reference.
        unsafe { pointer.write_volatile(words[index]) };
        index += 1;
    }
    Ok(())
}

pub(crate) fn zero_mapped_frame(mmu: &dyn Mmu, address: usize) -> Result<(), MmuError> {
    validate_mapped_frame(mmu, address, 0, ENTRIES)?;
    let _access = lock_memory();
    zero_page(address);
    Ok(())
}

pub(crate) fn mapped_atomic_words(
    mmu: &dyn Mmu,
    address: usize,
) -> Result<&[AtomicU64; PAGE_WORDS], MmuError> {
    validate_mapped_frame(mmu, address, 0, PAGE_WORDS)?;
    // SAFETY: The caller retains the writable normal memory mapping and excludes conflicting references and non-atomic accesses for the borrow.
    Ok(unsafe { &*(address as *const [AtomicU64; PAGE_WORDS]) })
}

pub(crate) fn zero_page(address: usize) {
    for offset in (0..PAGE_SIZE).step_by(size_of::<[u64; 8]>()) {
        // SAFETY: The caller owns the writable page exclusively; each store covers eight aligned words within it.
        unsafe { ((address + offset) as *mut [u64; 8]).write([0; 8]) };
    }
}

pub(crate) fn validate_physical<F: Format>(address: u64) -> Result<(), MmuError> {
    if address >= 1 << F::PHYSICAL_BITS {
        return Err(MmuError::InvalidAddress);
    }
    if !address.is_multiple_of(PAGE_SIZE as u64) {
        return Err(MmuError::InvalidAlignment);
    }
    Ok(())
}

pub(crate) fn is_canonical(address: usize, bits: u32) -> bool {
    (((address as i64) << (64 - bits)) >> (64 - bits)) as usize == address
}

pub(crate) fn validate_code_range(
    range: &core::ops::Range<usize>,
    bits: u32,
) -> Result<(), MmuError> {
    if range.start > range.end {
        return Err(MmuError::InvalidRange);
    }
    if range.is_empty() {
        return Ok(());
    }
    if !is_canonical(range.start, bits)
        || !is_canonical(range.end - 1, bits)
        || (range.start >> (bits - 1)) != ((range.end - 1) >> (bits - 1))
    {
        return Err(MmuError::InvalidAddress);
    }
    Ok(())
}

pub(crate) fn layout<F: Format>(format: &F) -> PageTableLayout {
    PageTableLayout {
        levels: F::LEVELS,
        index_bits: 9,
        window: TableWindow {
            virtual_address: PAGE_TABLE_WINDOW,
            table: Frame::new(format.window_table_address()).expect("unaligned table window"),
            level: 1,
        },
    }
}

fn validate_entry<F: Format>(index: usize, level: usize) -> Result<(), MmuError> {
    if index >= ENTRIES || level >= F::LEVELS {
        return Err(MmuError::InvalidRange);
    }
    Ok(())
}

pub(crate) fn read_entry<F: Format>(
    format: &F,
    table: Frame,
    index: usize,
    level: usize,
) -> Result<PageTableEntry, MmuError> {
    validate_entry::<F>(index, level)?;
    let mut words = [0];
    read_frame(format, table.address(), index, &mut words)?;
    decode_entry(format, words[0], level)
}

pub(crate) fn copy_kernel_entries<F: Format>(
    format: &F,
    source: Frame,
    destination: Frame,
) -> Result<(), MmuError> {
    let mut entries = [0; ENTRIES / 2];
    read_frame(format, source.address(), ENTRIES / 2, &mut entries)?;
    for &entry in &entries {
        if !matches!(
            decode_entry(format, entry, F::LEVELS - 1)?,
            PageTableEntry::Table(_)
        ) {
            return Err(MmuError::InvalidTable);
        }
    }
    write_frame(format, destination.address(), ENTRIES / 2, &entries)
}

fn decode_entry<F: Format>(
    format: &F,
    entry: u64,
    level: usize,
) -> Result<PageTableEntry, MmuError> {
    if entry == 0 {
        Ok(PageTableEntry::Empty)
    } else if level == 0 {
        format.decode_mapping(entry).map(PageTableEntry::Mapping)
    } else {
        let address = format.decode_table(entry)?;
        validate_physical::<F>(address)?;
        Ok(PageTableEntry::Table(
            Frame::new(address).ok_or(MmuError::InvalidAlignment)?,
        ))
    }
}

pub(crate) fn walk_mapped<F: Format>(
    format: &F,
    root: Frame,
    address: usize,
    physical_start: u64,
    mapping: Range<usize>,
) -> Result<PageWalk, MmuError> {
    validate_physical::<F>(root.address())?;
    if !format.is_valid_virtual_address(address) {
        return Err(MmuError::InvalidAddress);
    }
    let _access = lock_memory();
    let mut walk = PageWalk {
        tables: [None; MAX_PAGE_TABLE_LEVELS],
        level: F::LEVELS - 1,
        entry: PageTableEntry::Empty,
    };
    let mut table = root;
    loop {
        walk.tables[walk.level] = Some(table);
        let slot =
            (address >> (PAGE_SIZE.trailing_zeros() as usize + 9 * walk.level)) & (ENTRIES - 1);
        let offset = table
            .address()
            .checked_sub(physical_start)
            .and_then(|offset| usize::try_from(offset).ok())
            .ok_or(MmuError::InvalidAddress)?;
        let mapped = mapping
            .start
            .checked_add(offset)
            .filter(|mapped| {
                mapped
                    .checked_add(PAGE_SIZE)
                    .is_some_and(|end| end <= mapping.end)
            })
            .ok_or(MmuError::InvalidAddress)?;
        // SAFETY: The supplied mapping covers the validated table page; the access lock excludes software writes.
        let entry = unsafe { ((mapped + slot * size_of::<u64>()) as *const u64).read_volatile() };
        walk.entry = decode_entry(format, entry, walk.level)?;
        match walk.entry {
            PageTableEntry::Table(next) if walk.level > 0 => {
                table = next;
                walk.level -= 1;
            }
            _ => return Ok(walk),
        }
    }
}

pub(crate) fn table_is_empty<F: Format>(
    format: &F,
    table: Frame,
    level: usize,
) -> Result<bool, MmuError> {
    validate_entry::<F>(0, level)?;
    format.with_frame(table.address(), PagePermissions::ReadOnly, |address| {
        // SAFETY: The window maps the complete table read-only, and page table access is serialized by the caller.
        let page = unsafe { &*(address as *const PageTable) };
        for first in (0..ENTRIES).step_by(8) {
            let occupied = page.read(first)
                | page.read(first + 1)
                | page.read(first + 2)
                | page.read(first + 3)
                | page.read(first + 4)
                | page.read(first + 5)
                | page.read(first + 6)
                | page.read(first + 7);
            if occupied != 0 {
                for index in first..first + 8 {
                    let entry = page.read(index);
                    if entry != 0 {
                        decode_entry(format, entry, level)?;
                        return Ok(false);
                    }
                }
            }
        }
        Ok(true)
    })?
}

pub(crate) fn write_entry<F: Format>(
    format: &F,
    table: Frame,
    index: usize,
    level: usize,
    entry: PageTableEntry,
) -> Result<(), MmuError> {
    validate_entry::<F>(index, level)?;
    let encoded = match entry {
        PageTableEntry::Empty => 0,
        PageTableEntry::Table(next) if level > 0 => {
            validate_physical::<F>(next.address())?;
            let entry = format.encode_table(next.address());
            if entry == 0 {
                return Err(MmuError::InvalidTable);
            }
            entry
        }
        PageTableEntry::Mapping(mapping) if level == 0 => {
            validate_physical::<F>(mapping.physical_address)?;
            format.encode_mapping(mapping)?
        }
        _ => return Err(MmuError::InvalidTable),
    };
    format.with_table(table.address(), |table| table.write(index, encoded))
}
