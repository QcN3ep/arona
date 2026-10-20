// SPDX-License-Identifier: MPL-2.0

use core::arch::asm;
use core::ops::Range;

use kernel::hardware::mmu::{
    MemoryType, Mmu, MmuError, PageAttributes, PageMapping, PagePermissions, PageTableStorage,
};

use crate::paging::{self, Format};

// REFERENCE: https://loongson.github.io/LoongArch-Documentation/LoongArch-Vol1-EN.html
const VALID: u64 = 1;
const DIRTY: u64 = 1 << 1;
const USER: u64 = 3 << 2;
const CACHED: u64 = 1 << 4;
const NO_READ: u64 = 1 << 61;
const NO_EXECUTE: u64 = 1 << 62;
const PHYSICAL: u64 = ((1 << 48) - 1) & !0xfff;
const PWCL: usize = 12 | (9 << 5) | (21 << 10) | (9 << 15) | (30 << 20) | (9 << 25);
const PWCH: usize = 39 | (9 << 6);

crate::declare_hardware!(
    static MMU: LoongArchMmu = LoongArchMmu => kernel::hardware::mmu::bind
);

pub struct LoongArchMmu;

impl Format for LoongArchMmu {
    const LEVELS: usize = 4;
    const PHYSICAL_BITS: u32 = 48;

    fn encode_table(&self, address: u64) -> u64 {
        address
    }

    fn decode_table(&self, entry: u64) -> Result<u64, MmuError> {
        if entry & !PHYSICAL != 0 {
            return Err(MmuError::InvalidTable);
        }
        Ok(entry)
    }

    fn encode_mapping(&self, mapping: PageMapping) -> Result<u64, MmuError> {
        let attributes = mapping.attributes;
        let permissions = match attributes.permissions {
            PagePermissions::ReadOnly => NO_EXECUTE,
            PagePermissions::ReadWrite => DIRTY | NO_EXECUTE,
            PagePermissions::ReadExecute => 0,
            PagePermissions::ExecuteOnly => NO_READ,
        };
        let memory_type = match attributes.memory_type {
            MemoryType::Normal => CACHED,
            MemoryType::Device if permissions & NO_EXECUTE == 0 => {
                return Err(MmuError::InvalidAttributes);
            }
            MemoryType::Device => 0,
        };
        Ok(mapping.physical_address
            | VALID
            | permissions
            | memory_type
            | if attributes.user_accessible { USER } else { 0 })
    }

    fn decode_mapping(&self, entry: u64) -> Result<PageMapping, MmuError> {
        if entry & VALID == 0
            || entry & !(PHYSICAL | VALID | DIRTY | USER | CACHED | NO_READ | NO_EXECUTE) != 0
        {
            return Err(MmuError::InvalidTable);
        }
        let permissions = match entry & (DIRTY | NO_READ | NO_EXECUTE) {
            NO_EXECUTE => PagePermissions::ReadOnly,
            flags if flags == DIRTY | NO_EXECUTE => PagePermissions::ReadWrite,
            0 => PagePermissions::ReadExecute,
            NO_READ => PagePermissions::ExecuteOnly,
            _ => return Err(MmuError::InvalidAttributes),
        };
        let user_accessible = match entry & USER {
            0 => false,
            USER => true,
            _ => return Err(MmuError::InvalidAttributes),
        };
        let memory_type = match entry & CACHED {
            CACHED => MemoryType::Normal,
            0 if entry & NO_EXECUTE != 0 => MemoryType::Device,
            _ => return Err(MmuError::InvalidAttributes),
        };
        Ok(PageMapping {
            physical_address: entry & PHYSICAL,
            attributes: PageAttributes {
                permissions,
                memory_type,
                user_accessible,
            },
        })
    }
}

impl Mmu for LoongArchMmu {
    fn is_valid_virtual_address(&self, address: usize) -> bool {
        paging::is_canonical(address, 48)
    }

    fn initialize(&self, tables: &mut dyn PageTableStorage) -> Result<(), MmuError> {
        paging::initialize(self, tables)
    }

    fn map(
        &self,
        tables: &mut dyn PageTableStorage,
        address: usize,
        mapping: PageMapping,
    ) -> Result<(), MmuError> {
        paging::map(self, tables, address, mapping)
    }

    fn unmap(
        &self,
        tables: &mut dyn PageTableStorage,
        address: usize,
    ) -> Result<PageMapping, MmuError> {
        paging::unmap(self, tables, address)
    }

    fn protect(
        &self,
        tables: &mut dyn PageTableStorage,
        address: usize,
        permissions: PagePermissions,
    ) -> Result<(), MmuError> {
        paging::protect(self, tables, address, permissions)
    }

    fn query(
        &self,
        tables: &dyn PageTableStorage,
        address: usize,
    ) -> Result<Option<PageMapping>, MmuError> {
        paging::query(self, tables, address)
    }

    fn switch_root(&self, tables: &dyn PageTableStorage) -> Result<(), MmuError> {
        let root = tables.root_address();
        paging::validate_physical::<Self>(root)?;
        if root == 0 {
            return Err(MmuError::InvalidTable);
        }
        tables.table(root)?;
        let refill = refill_entry();
        let mapping = self.query(tables, refill)?.ok_or(MmuError::NotMapped)?;
        if !matches!(
            mapping.attributes.permissions,
            PagePermissions::ReadExecute | PagePermissions::ExecuteOnly
        ) || mapping.attributes.memory_type != MemoryType::Normal
            || mapping.attributes.user_accessible
        {
            return Err(MmuError::InvalidAttributes);
        }
        let physical_refill = mapping.physical_address as usize | (refill & 0xfff);
        // SAFETY: 新页表须覆盖当前代码、栈和活动数据，中断须关闭且后备页须持续有效。
        unsafe {
            asm!(
                "dbar 0",
                "csrwr {refill}, 0x88",
                "csrwr {pwcl}, 0x1c",
                "csrwr {pwch}, 0x1d",
                "csrwr {page_size}, 0x1e",
                "csrwr {refill_size}, 0x8e",
                "csrwr {low}, 0x19",
                "csrwr {high}, 0x1a",
                "csrwr $zero, 0x18",
                "csrwr $zero, 0x180",
                "csrwr $zero, 0x181",
                "csrwr $zero, 0x182",
                "csrwr $zero, 0x183",
                "invtlb 0, $zero, $zero",
                "ibar 0",
                refill = inout(reg) physical_refill => _,
                pwcl = inout(reg) PWCL => _,
                pwch = inout(reg) PWCH => _,
                page_size = inout(reg) 12usize => _,
                refill_size = inout(reg) 12usize => _,
                low = inout(reg) root as usize => _,
                high = inout(reg) root as usize => _,
                options(nostack),
            );
        }
        Ok(())
    }

    fn invalidate_page(&self, address: usize) -> Result<(), MmuError> {
        if !self.is_valid_virtual_address(address) {
            return Err(MmuError::InvalidAddress);
        }
        // SAFETY: 处理器须允许访问 ASID 并执行本地 TLB 失效。
        unsafe {
            asm!(
                "dbar 0",
                "csrrd {asid}, 0x18",
                "bstrpick.d {asid}, {asid}, 9, 0",
                "invtlb 6, {asid}, {address}",
                "ibar 0",
                asid = out(reg) _,
                address = in(reg) address,
                options(nostack),
            );
        }
        Ok(())
    }

    fn invalidate_all(&self) {
        // SAFETY: 处理器须允许执行本地 TLB 失效指令。
        unsafe {
            asm!(
                "dbar 0",
                "invtlb 0, $zero, $zero",
                "ibar 0",
                options(nostack)
            )
        };
    }

    fn synchronize_code(&self, range: Range<usize>) -> Result<(), MmuError> {
        paging::validate_code_range(&range, 48)?;
        if range.is_empty() {
            return Ok(());
        }
        // SAFETY: 范围须为保持有效的普通内存映射，且同步期间不得写入。
        unsafe {
            asm!("dbar 0", options(nostack));
            cache_lines::<0x11>(&range, cache_line_size(0x12));
            asm!("dbar 0", options(nostack));
            cache_lines::<0x10>(&range, cache_line_size(0x11));
            asm!("dbar 0", "ibar 0", options(nostack));
        }
        Ok(())
    }
}

fn cache_line_size(index: usize) -> usize {
    let configuration: usize;
    // SAFETY: CPUCFG 的索引须对应 LA464 的一级缓存描述。
    unsafe {
        asm!("cpucfg {value}, {index}", value = out(reg) configuration, index = in(reg) index, options(nomem, nostack))
    };
    1 << ((configuration >> 24) & 0x7f)
}

/// # Safety
///
/// range 须为有效普通内存映射，line_size 须为目标一级缓存行大小。
unsafe fn cache_lines<const OPERATION: u32>(range: &Range<usize>, line_size: usize) {
    let mut address = range.start & !(line_size - 1);
    loop {
        // SAFETY: 地址须对应 range 覆盖的有效缓存行，操作码须选择对应一级缓存。
        unsafe {
            asm!("cacop {operation}, {address}, 0", operation = const OPERATION, address = in(reg) address, options(nostack))
        };
        if range.end - address <= line_size {
            break;
        }
        address += line_size;
    }
}

fn refill_entry() -> usize {
    let entry: usize;
    // SAFETY: refill 入口须在物理地址执行，不依赖栈或虚拟映射；页表根和遍历参数须有效。
    unsafe {
        asm!(
            "la.local {entry}, 2f",
            "b 4f",
            ".balign 128",
            "2:",
            "csrwr $t0, 0x8b",
            "csrrd $t0, 0x1b",
            "beqz $t0, 3f",
            "lddir $t0, $t0, 3",
            "beqz $t0, 3f",
            "lddir $t0, $t0, 2",
            "beqz $t0, 3f",
            "lddir $t0, $t0, 1",
            "beqz $t0, 3f",
            "ldpte $t0, 0",
            "ldpte $t0, 1",
            "tlbfill",
            "csrrd $t0, 0x8b",
            "ertn",
            "3:",
            "idle 0",
            "b 3b",
            "4:",
            entry = out(reg) entry,
            options(nomem, nostack),
        );
    }
    entry
}
