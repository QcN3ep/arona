// SPDX-License-Identifier: MPL-2.0

use core::arch::asm;
use core::ops::Range;

use kernel::hardware::mmu::{
    MemoryType, Mmu, MmuError, PageAttributes, PageMapping, PagePermissions, PageTableFrames,
};

use crate::paging::{
    self, Format, PAGE_TABLE_WINDOW, PAGE_TABLE_WINDOW_TABLE, PageTable, WINDOW_INDEX,
};

// REFERENCE: https://docs.riscv.org/reference/isa/priv/supervisor.html
const VALID: u64 = 1;
const READ: u64 = 1 << 1;
const WRITE: u64 = 1 << 2;
const EXECUTE: u64 = 1 << 3;
const USER: u64 = 1 << 4;
const ACCESSED: u64 = 1 << 6;
const DIRTY: u64 = 1 << 7;
const PPN: u64 = ((1 << 44) - 1) << 10;

fn invalidate_address(address: usize) {
    // SAFETY: address 须为当前地址空间中的规范虚拟地址。
    unsafe { asm!("sfence.vma {address}, zero", address = in(reg) address, options(nostack)) };
}

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

    fn with_table<T>(
        &self,
        address: u64,
        operation: impl FnOnce(&mut PageTable) -> T,
    ) -> Result<T, MmuError> {
        paging::validate_physical::<Self>(address)?;
        let entry = self.encode_mapping(PageMapping {
            physical_address: address,
            attributes: PageAttributes {
                permissions: PagePermissions::ReadWrite,
                memory_type: MemoryType::Normal,
                user_accessible: false,
            },
        })?;
        let table = core::ptr::addr_of_mut!(PAGE_TABLE_WINDOW_TABLE);
        // SAFETY: 调用方须串行化所有处理器上的 MMU 操作。
        unsafe {
            if (*table).read(WINDOW_INDEX) != 0 {
                return Err(MmuError::InvalidTable);
            }
            (*table).write(WINDOW_INDEX, entry);
        }
        invalidate_address(PAGE_TABLE_WINDOW);
        // SAFETY: 当前根须将 PAGE_TABLE_WINDOW 映射到保留的页表窗口。
        let result = unsafe { operation(&mut *(PAGE_TABLE_WINDOW as *mut PageTable)) };
        // SAFETY: 调用方须串行化所有处理器上的 MMU 操作。
        unsafe { (*table).write(WINDOW_INDEX, 0) };
        invalidate_address(PAGE_TABLE_WINDOW);
        Ok(result)
    }
}

impl Mmu for Sv39 {
    fn is_valid_virtual_address(&self, address: usize) -> bool {
        paging::is_canonical(address, 39)
    }

    fn initialize(&self, frames: &mut dyn PageTableFrames) -> Result<(), MmuError> {
        paging::initialize(self, frames)
    }

    fn map(
        &self,
        frames: &mut dyn PageTableFrames,
        address: usize,
        mapping: PageMapping,
    ) -> Result<(), MmuError> {
        paging::map(self, frames, address, mapping)
    }

    fn unmap(
        &self,
        frames: &mut dyn PageTableFrames,
        address: usize,
    ) -> Result<PageMapping, MmuError> {
        paging::unmap(self, frames, address)
    }

    fn protect(
        &self,
        frames: &mut dyn PageTableFrames,
        address: usize,
        permissions: PagePermissions,
    ) -> Result<(), MmuError> {
        paging::protect(self, frames, address, permissions)
    }

    fn query(
        &self,
        frames: &dyn PageTableFrames,
        address: usize,
    ) -> Result<Option<PageMapping>, MmuError> {
        paging::query(self, frames, address)
    }

    fn switch_root(&self, frames: &dyn PageTableFrames) -> Result<(), MmuError> {
        let root = frames.root_address();
        paging::validate_physical::<Self>(root)?;
        paging::validate_window(self, frames)?;
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
        invalidate_address(address);
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
