// SPDX-License-Identifier: MPL-2.0

use kernel::hardware::mmu::{
    Mmu, MmuError, PAGE_SIZE, PageMapping, PagePermissions, PageTableStorage,
};

const ENTRIES: usize = PAGE_SIZE / size_of::<u64>();
const MAX_LEVELS: usize = 4;

pub(crate) trait Format: Mmu {
    const LEVELS: usize;
    const PHYSICAL_BITS: u32;

    fn encode_table(&self, address: u64) -> u64;
    fn decode_table(&self, entry: u64) -> Result<u64, MmuError>;
    fn encode_mapping(&self, mapping: PageMapping) -> Result<u64, MmuError>;
    fn decode_mapping(&self, entry: u64) -> Result<PageMapping, MmuError>;
}

#[repr(C, align(4096))]
struct PageTable {
    entries: [u64; ENTRIES],
}

struct Walk {
    tables: [u64; MAX_LEVELS],
    level: usize,
    entry: u64,
}

impl PageTable {
    fn get(storage: &dyn PageTableStorage, address: u64) -> Result<&Self, MmuError> {
        let bytes = storage.table(address)?;
        if bytes.as_ptr().align_offset(align_of::<Self>()) != 0 {
            return Err(MmuError::InvalidAlignment);
        }
        // SAFETY: 后备页须固定有效，PageTable 与该页同尺寸同对齐且全部位模式有效。
        Ok(unsafe { &*bytes.as_ptr().cast::<Self>() })
    }

    fn get_mut(storage: &mut dyn PageTableStorage, address: u64) -> Result<&mut Self, MmuError> {
        let bytes = storage.table_mut(address)?;
        if bytes.as_ptr().align_offset(align_of::<Self>()) != 0 {
            return Err(MmuError::InvalidAlignment);
        }
        // SAFETY: 后备页须固定且独占有效，PageTable 与该页同尺寸同对齐且全部位模式有效。
        Ok(unsafe { &mut *bytes.as_mut_ptr().cast::<Self>() })
    }

    fn read(&self, index: usize) -> u64 {
        // SAFETY: index 须在页表内，硬件与其他处理器不得并发修改该条目。
        unsafe { (&raw const self.entries[index]).read_volatile() }
    }

    fn write(&mut self, index: usize, entry: u64) {
        // SAFETY: index 须在独占页表内，条目以对齐的完整 64 位存储提交。
        unsafe { (&raw mut self.entries[index]).write_volatile(entry) }
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

pub(crate) fn validate_page(mmu: &dyn Mmu, address: usize) -> Result<(), MmuError> {
    if !mmu.is_valid_virtual_address(address) {
        return Err(MmuError::InvalidAddress);
    }
    if !address.is_multiple_of(PAGE_SIZE) {
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

pub(crate) fn initialize<F: Format>(
    format: &F,
    storage: &mut dyn PageTableStorage,
) -> Result<(), MmuError> {
    let root = storage.root_address();
    validate_physical::<F>(root)?;
    if format.encode_table(root) == 0 {
        return Err(MmuError::InvalidTable);
    }
    let root = PageTable::get_mut(storage, root)?;
    for index in 0..ENTRIES {
        root.write(index, 0);
    }
    Ok(())
}

fn index(address: usize, level: usize) -> usize {
    (address >> (12 + 9 * level)) & (ENTRIES - 1)
}

fn walk<F: Format>(
    format: &F,
    storage: &dyn PageTableStorage,
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
    let mut table = storage.root_address();
    loop {
        validate_physical::<F>(table)?;
        if format.encode_table(table) == 0 {
            return Err(MmuError::InvalidTable);
        }
        if walk.tables[walk.level + 1..F::LEVELS].contains(&table) {
            return Err(MmuError::InvalidTable);
        }
        walk.tables[walk.level] = table;
        walk.entry = PageTable::get(storage, table)?.read(index(address, walk.level));
        if walk.entry == 0 || walk.level == 0 {
            return Ok(walk);
        }
        table = format.decode_table(walk.entry)?;
        walk.level -= 1;
    }
}

pub(crate) fn map<F: Format>(
    format: &F,
    storage: &mut dyn PageTableStorage,
    address: usize,
    mapping: PageMapping,
) -> Result<(), MmuError> {
    validate_page(format, address)?;
    validate_physical::<F>(mapping.physical_address)?;
    let mut entry = format.encode_mapping(mapping)?;
    let walk = walk(format, storage, address)?;
    if walk.entry != 0 {
        format.decode_mapping(walk.entry)?;
        return Err(MmuError::AlreadyMapped);
    }

    let mut spare = [0; MAX_LEVELS - 1];
    let mut count = 0;
    let result = (|| {
        for level in 0..walk.level {
            let table = storage.take_table()?;
            spare[count] = table;
            count += 1;
            validate_physical::<F>(table)?;
            let branch = format.encode_table(table);
            if branch == 0 {
                return Err(MmuError::InvalidTable);
            }
            PageTable::get_mut(storage, table)?.write(index(address, level), entry);
            entry = branch;
        }
        PageTable::get_mut(storage, walk.tables[walk.level])?
            .write(index(address, walk.level), entry);
        Ok(())
    })();
    if result.is_err() {
        for table in spare[..count].iter().rev() {
            // SAFETY: 这些页须由本次借出，且尚未向页表树发布。
            storage.return_table(*table);
        }
        return result;
    }
    format.invalidate_all();
    Ok(())
}

pub(crate) fn unmap<F: Format>(
    format: &F,
    storage: &mut dyn PageTableStorage,
    address: usize,
) -> Result<PageMapping, MmuError> {
    validate_page(format, address)?;
    let walk = walk(format, storage, address)?;
    if walk.entry == 0 {
        return Err(MmuError::NotMapped);
    }
    let mapping = format.decode_mapping(walk.entry)?;
    let mut detach = 0;
    while detach + 1 < F::LEVELS {
        let table = PageTable::get(storage, walk.tables[detach])?;
        if (0..ENTRIES).any(|slot| slot != index(address, detach) && table.read(slot) != 0) {
            break;
        }
        detach += 1;
    }
    PageTable::get_mut(storage, walk.tables[detach])?.write(index(address, detach), 0);
    format.invalidate_all();
    for table in &walk.tables[..detach] {
        // SAFETY: 空页表子树须已断开，且本地硬件引用已失效。
        storage.return_table(*table);
    }
    Ok(mapping)
}

pub(crate) fn protect<F: Format>(
    format: &F,
    storage: &mut dyn PageTableStorage,
    address: usize,
    permissions: PagePermissions,
) -> Result<(), MmuError> {
    validate_page(format, address)?;
    let walk = walk(format, storage, address)?;
    if walk.entry == 0 {
        return Err(MmuError::NotMapped);
    }
    let mut mapping = format.decode_mapping(walk.entry)?;
    mapping.attributes.permissions = permissions;
    let entry = format.encode_mapping(mapping)?;
    PageTable::get_mut(storage, walk.tables[0])?.write(index(address, 0), entry);
    format.invalidate_all();
    Ok(())
}

pub(crate) fn query<F: Format>(
    format: &F,
    storage: &dyn PageTableStorage,
    address: usize,
) -> Result<Option<PageMapping>, MmuError> {
    let walk = walk(format, storage, address)?;
    if walk.entry == 0 {
        return Ok(None);
    }
    format.decode_mapping(walk.entry).map(Some)
}
