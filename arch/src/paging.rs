// SPDX-License-Identifier: MPL-2.0

use kernel::hardware::mmu::{
    Mmu, MmuError, PAGE_SIZE, PageMapping, PagePermissions, PageTableFrames,
};

const ENTRIES: usize = PAGE_SIZE / size_of::<u64>();
const MAX_LEVELS: usize = 4;
pub(crate) const WINDOW_INDEX: usize = ENTRIES - 1;
const WINDOW_REGION_SIZE: usize = 1 << 21;
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
    ) -> Result<T, MmuError>;
}

#[repr(C, align(4096))]
pub(crate) struct PageTable {
    entries: [u64; ENTRIES],
}

#[used]
#[unsafe(link_section = ".boot_tables.window")]
pub(crate) static mut PAGE_TABLE_WINDOW_TABLE: PageTable = PageTable::new();

struct Walk {
    tables: [u64; MAX_LEVELS],
    level: usize,
    entry: u64,
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
    format.with_table(address, |page| {
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

pub(crate) fn validate_page(mmu: &dyn Mmu, address: usize) -> Result<(), MmuError> {
    if !mmu.is_valid_virtual_address(address) {
        return Err(MmuError::InvalidAddress);
    }
    if !address.is_multiple_of(PAGE_SIZE) {
        return Err(MmuError::InvalidAlignment);
    }
    if address & !(WINDOW_REGION_SIZE - 1) == PAGE_TABLE_WINDOW & !(WINDOW_REGION_SIZE - 1) {
        return Err(MmuError::InvalidAddress);
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

pub(crate) fn initialize<F: Format>(
    format: &F,
    frames: &mut dyn PageTableFrames,
) -> Result<(), MmuError> {
    let root = frames.root_address();
    validate_physical::<F>(root)?;
    if format.encode_table(root) == 0 {
        return Err(MmuError::InvalidTable);
    }

    let window = format.window_table_address();
    validate_physical::<F>(window)?;
    if root == window {
        return Err(MmuError::InvalidTable);
    }
    let mut allocated = [0; MAX_LEVELS - 2];
    let mut count = 0;
    let result = (|| {
        while count < F::LEVELS - 2 {
            let table = frames.take_table()?;
            if table == root || table == window || allocated[..count].contains(&table) {
                return Err(MmuError::InvalidTable);
            }
            allocated[count] = table;
            count += 1;
            validate_physical::<F>(table)?;
            if format.encode_table(table) == 0 {
                return Err(MmuError::InvalidTable);
            }
        }
        for table in &allocated[..count] {
            format.with_table(*table, PageTable::clear)?;
        }
        for (index, table) in allocated[..count].iter().enumerate() {
            let next = allocated
                .get(index + 1)
                .copied()
                .filter(|_| index + 1 < count)
                .unwrap_or(window);
            format.with_table(*table, |page| {
                page.write(WINDOW_INDEX, format.encode_table(next));
            })?;
        }
        format.with_table(root, |page| {
            page.clear();
            page.write(WINDOW_INDEX, format.encode_table(allocated[0]));
        })?;
        Ok(())
    })();
    if result.is_err() {
        for table in allocated[..count].iter().rev() {
            // SAFETY: 初始化失败时这些页尚未从根页可达。
            frames.return_table(*table);
        }
    }
    result
}

pub(crate) fn validate_window<F: Format>(
    format: &F,
    frames: &dyn PageTableFrames,
) -> Result<(), MmuError> {
    let walk = walk(format, frames, PAGE_TABLE_WINDOW)?;
    if walk.tables[0] != format.window_table_address() {
        return Err(MmuError::InvalidTable);
    }
    Ok(())
}

fn index(address: usize, level: usize) -> usize {
    (address >> (12 + 9 * level)) & (ENTRIES - 1)
}

fn walk<F: Format>(
    format: &F,
    frames: &dyn PageTableFrames,
    address: usize,
) -> Result<Walk, MmuError> {
    if !format.is_valid_virtual_address(address) {
        return Err(MmuError::InvalidAddress);
    }
    let mut walk = Walk {
        tables: [0; MAX_LEVELS],
        level: F::LEVELS - 1,
        entry: 0,
    };
    let mut table = frames.root_address();
    loop {
        validate_physical::<F>(table)?;
        if format.encode_table(table) == 0 {
            return Err(MmuError::InvalidTable);
        }
        if walk.tables[walk.level + 1..F::LEVELS].contains(&table) {
            return Err(MmuError::InvalidTable);
        }
        walk.tables[walk.level] = table;
        walk.entry = format.with_table(table, |page| page.read(index(address, walk.level)))?;
        if walk.entry == 0 || walk.level == 0 {
            return Ok(walk);
        }
        table = format.decode_table(walk.entry)?;
        walk.level -= 1;
    }
}

pub(crate) fn map<F: Format>(
    format: &F,
    frames: &mut dyn PageTableFrames,
    address: usize,
    mapping: PageMapping,
) -> Result<(), MmuError> {
    validate_page(format, address)?;
    validate_physical::<F>(mapping.physical_address)?;
    let mut entry = format.encode_mapping(mapping)?;
    let walk = walk(format, frames, address)?;
    if walk.entry != 0 {
        format.decode_mapping(walk.entry)?;
        return Err(MmuError::AlreadyMapped);
    }

    let mut spare = [0; MAX_LEVELS - 1];
    let mut count = 0;
    let result = (|| {
        for level in 0..walk.level {
            let table = frames.take_table()?;
            spare[count] = table;
            count += 1;
            validate_physical::<F>(table)?;
            let branch = format.encode_table(table);
            if branch == 0 {
                return Err(MmuError::InvalidTable);
            }
            format.with_table(table, |page| {
                page.clear();
                page.write(index(address, level), entry);
            })?;
            entry = branch;
        }
        format.with_table(walk.tables[walk.level], |page| {
            page.write(index(address, walk.level), entry);
        })?;
        Ok(())
    })();
    if result.is_err() {
        for table in spare[..count].iter().rev() {
            // SAFETY: 这些帧由本次借出，且尚未向页表树发布。
            frames.return_table(*table);
        }
        return result;
    }
    format.invalidate_all();
    Ok(())
}

pub(crate) fn unmap<F: Format>(
    format: &F,
    frames: &mut dyn PageTableFrames,
    address: usize,
) -> Result<PageMapping, MmuError> {
    validate_page(format, address)?;
    let walk = walk(format, frames, address)?;
    if walk.entry == 0 {
        return Err(MmuError::NotMapped);
    }
    let mapping = format.decode_mapping(walk.entry)?;
    let mut detach = 0;
    while detach + 1 < F::LEVELS {
        let occupied = format.with_table(walk.tables[detach], |table| {
            (0..ENTRIES).any(|slot| slot != index(address, detach) && table.read(slot) != 0)
        })?;
        if occupied {
            break;
        }
        detach += 1;
    }
    format.with_table(walk.tables[detach], |table| {
        table.write(index(address, detach), 0);
    })?;
    format.invalidate_all();
    for table in &walk.tables[..detach] {
        // SAFETY: 空页表子树已断开，且本地硬件引用已失效。
        frames.return_table(*table);
    }
    Ok(mapping)
}

pub(crate) fn protect<F: Format>(
    format: &F,
    frames: &mut dyn PageTableFrames,
    address: usize,
    permissions: PagePermissions,
) -> Result<(), MmuError> {
    validate_page(format, address)?;
    let walk = walk(format, frames, address)?;
    if walk.entry == 0 {
        return Err(MmuError::NotMapped);
    }
    let mut mapping = format.decode_mapping(walk.entry)?;
    mapping.attributes.permissions = permissions;
    let entry = format.encode_mapping(mapping)?;
    format.with_table(walk.tables[0], |table| {
        table.write(index(address, 0), entry);
    })?;
    format.invalidate_all();
    Ok(())
}

pub(crate) fn query<F: Format>(
    format: &F,
    frames: &dyn PageTableFrames,
    address: usize,
) -> Result<Option<PageMapping>, MmuError> {
    let walk = walk(format, frames, address)?;
    if walk.entry == 0 {
        return Ok(None);
    }
    format.decode_mapping(walk.entry).map(Some)
}
