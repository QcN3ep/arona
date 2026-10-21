// SPDX-License-Identifier: MPL-2.0

use core::sync::atomic::{AtomicBool, Ordering};
use kernel::{
    hardware::mmu::{
        Mmu, MmuError, PAGE_SIZE, PageMapping, PagePermissions, PageTableEntry, PageTableLayout,
        TableWindow,
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
            // SAFETY: 窗口须独占映射完整可写页框。
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

static WINDOW_LOCK: AtomicBool = AtomicBool::new(false);

pub(crate) struct WindowGuard;

pub(crate) fn lock_window() -> WindowGuard {
    while WINDOW_LOCK
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    WindowGuard
}

impl Drop for WindowGuard {
    fn drop(&mut self) {
        WINDOW_LOCK.store(false, Ordering::Release);
    }
}

impl PageTable {
    const fn new() -> Self {
        Self {
            entries: [0; ENTRIES],
        }
    }

    pub(crate) fn clear(&mut self) {
        for index in 0..ENTRIES {
            self.write(index, 0);
        }
    }

    pub(crate) fn read(&self, index: usize) -> u64 {
        // SAFETY: index 须在页表内，硬件与其他处理器不得并发修改该条目。
        unsafe { (&raw const self.entries[index]).read_volatile() }
    }

    pub(crate) fn write(&mut self, index: usize, entry: u64) {
        // SAFETY: index 须在独占页表内，条目以对齐的完整 64 位存储提交。
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
        // SAFETY: 窗口须只读映射完整页框，访问期间不得与写引用冲突。
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
    let entry = words[0];
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
