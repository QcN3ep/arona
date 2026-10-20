// SPDX-License-Identifier: MPL-2.0

use core::arch::asm;
use core::ops::Range;

use kernel::hardware::mmu::{
    MemoryType, Mmu, MmuError, PageAttributes, PageMapping, PagePermissions, PageTableStorage,
};

use crate::paging::{self, Format};

// REFERENCE: https://docs.riscv.org/reference/isa/priv/supervisor.html
const VALID: u64 = 1;
const READ: u64 = 1 << 1;
const WRITE: u64 = 1 << 2;
const EXECUTE: u64 = 1 << 3;
const USER: u64 = 1 << 4;
const ACCESSED: u64 = 1 << 6;
const DIRTY: u64 = 1 << 7;
const PPN: u64 = ((1 << 44) - 1) << 10;

crate::declare_hardware!(
    static MMU: Sv39 = Sv39 => kernel::hardware::mmu::bind
);

pub struct Sv39;

impl Format for Sv39 {
    const LEVELS: usize = 3;
    const PHYSICAL_BITS: u32 = 56;

    fn encode_table(&self, address: u64) -> u64 {
        (address >> 2) | VALID
    }

    fn decode_table(&self, entry: u64) -> Result<u64, MmuError> {
        if entry & !PPN != VALID {
            return Err(MmuError::InvalidTable);
        }
        Ok((entry & PPN) << 2)
    }

    fn encode_mapping(&self, mapping: PageMapping) -> Result<u64, MmuError> {
        let attributes = mapping.attributes;
        let permissions = match attributes.permissions {
            PagePermissions::ReadOnly => READ,
            PagePermissions::ReadWrite => READ | WRITE | DIRTY,
            PagePermissions::ReadExecute => READ | EXECUTE,
            PagePermissions::ExecuteOnly => EXECUTE,
        };
        if attributes.memory_type == MemoryType::Device {
            return Err(if permissions & EXECUTE != 0 {
                MmuError::InvalidAttributes
            } else {
                MmuError::Unsupported
            });
        }
        Ok((mapping.physical_address >> 2)
            | VALID
            | ACCESSED
            | permissions
            | if attributes.user_accessible { USER } else { 0 })
    }

    fn decode_mapping(&self, entry: u64) -> Result<PageMapping, MmuError> {
        if entry & VALID == 0
            || entry & !(PPN | VALID | READ | WRITE | EXECUTE | USER | ACCESSED | DIRTY) != 0
        {
            return Err(MmuError::InvalidTable);
        }
        let permissions = match entry & (READ | WRITE | EXECUTE) {
            READ => PagePermissions::ReadOnly,
            flags if flags == READ | WRITE => PagePermissions::ReadWrite,
            flags if flags == READ | EXECUTE => PagePermissions::ReadExecute,
            EXECUTE => PagePermissions::ExecuteOnly,
            _ => return Err(MmuError::InvalidAttributes),
        };
        Ok(PageMapping {
            physical_address: (entry & PPN) << 2,
            attributes: PageAttributes {
                permissions,
                memory_type: MemoryType::Normal,
                user_accessible: entry & USER != 0,
            },
        })
    }
}

impl Mmu for Sv39 {
    fn is_valid_virtual_address(&self, address: usize) -> bool {
        paging::is_canonical(address, 39)
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
        tables.table(root)?;
        let satp = (8usize << 60) | (root as usize >> 12);
        // SAFETY: 新页表须覆盖当前代码、栈和活动数据，中断须关闭且后备页须持续有效。
        unsafe {
            asm!(
                "sfence.vma",
                "csrw satp, {root}",
                "sfence.vma",
                root = in(reg) satp,
                options(nostack),
            );
        }
        Ok(())
    }

    fn invalidate_page(&self, address: usize) -> Result<(), MmuError> {
        if !self.is_valid_virtual_address(address) {
            return Err(MmuError::InvalidAddress);
        }
        // SAFETY: 处理器须允许执行 S 模式地址翻译同步指令。
        unsafe { asm!("sfence.vma {address}, zero", address = in(reg) address, options(nostack)) };
        Ok(())
    }

    fn invalidate_all(&self) {
        // SAFETY: 处理器须允许执行 S 模式地址翻译同步指令。
        unsafe { asm!("sfence.vma", options(nostack)) };
    }

    fn synchronize_code(&self, range: Range<usize>) -> Result<(), MmuError> {
        paging::validate_code_range(&range, 39)?;
        if !range.is_empty() {
            // SAFETY: 范围须为有效普通内存映射，且此前写入须已完成。
            unsafe { asm!("fence.i", options(nostack)) };
        }
        Ok(())
    }
}
