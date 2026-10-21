// SPDX-License-Identifier: MPL-2.0

use core::arch::{asm, naked_asm};
use core::ops::Range;

use kernel::hardware::mmu::{
    MemoryType, Mmu, MmuError, PageAttributes, PageMapping, PagePermissions, PageTableEntry,
    PageTableLayout,
};

use kernel::memory::Frame;

use crate::paging::{
    self, Format, PAGE_TABLE_WINDOW, PAGE_TABLE_WINDOW_TABLE, PageTable, WINDOW_INDEX,
};

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

fn invalidate_address(address: usize) {
    // SAFETY: address 须为当前地址空间中的规范虚拟地址。
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
}

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

    fn with_frame<T>(
        &self,
        address: u64,
        permissions: PagePermissions,
        operation: impl FnOnce(usize) -> T,
    ) -> Result<T, MmuError> {
        paging::validate_physical::<Self>(address)?;
        let _access = paging::lock_memory();
        let entry = self.encode_mapping(PageMapping {
            physical_address: address,
            attributes: PageAttributes {
                permissions,
                memory_type: MemoryType::Normal,
                user_accessible: false,
            },
        })?;
        let table = core::ptr::addr_of_mut!(PAGE_TABLE_WINDOW_TABLE);
        // SAFETY: 须持有物理访问窗口锁。
        unsafe {
            if (*table).read(WINDOW_INDEX) != 0 {
                return Err(MmuError::InvalidTable);
            }
            (*table).write(WINDOW_INDEX, entry);
        }
        invalidate_address(PAGE_TABLE_WINDOW);
        let result = operation(PAGE_TABLE_WINDOW);
        // SAFETY: 须持有物理访问窗口锁。
        unsafe { (*table).write(WINDOW_INDEX, 0) };
        invalidate_address(PAGE_TABLE_WINDOW);
        Ok(result)
    }
}

impl Mmu for LoongArchMmu {
    fn is_valid_virtual_address(&self, address: usize) -> bool {
        paging::is_canonical(address, 48)
    }

    fn physical_address_bits(&self) -> u32 {
        Self::PHYSICAL_BITS
    }

    fn layout(&self) -> PageTableLayout {
        paging::layout(self)
    }

    fn read_entry(
        &self,
        table: Frame,
        index: usize,
        level: usize,
    ) -> Result<PageTableEntry, MmuError> {
        paging::read_entry(self, table, index, level)
    }

    fn write_entry(
        &self,
        table: Frame,
        index: usize,
        level: usize,
        entry: PageTableEntry,
    ) -> Result<(), MmuError> {
        paging::write_entry(self, table, index, level, entry)
    }

    fn switch_root(&self, root: Frame) -> Result<(), MmuError> {
        let _access = paging::lock_memory();
        let root = root.address();
        paging::validate_physical::<Self>(root)?;
        if root == 0 {
            return Err(MmuError::InvalidTable);
        }
        // SAFETY: 新页表须覆盖当前代码、栈和活动数据，中断须关闭且后备页须持续有效。
        unsafe {
            asm!(
                "dbar 0",
                "la.abs {refill}, {refill_entry}",
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
                refill = out(reg) _,
                refill_entry = sym refill,
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

    fn read_frame(
        &self,
        physical_address: u64,
        offset: usize,
        words: &mut [u64],
    ) -> Result<(), MmuError> {
        paging::read_frame(self, physical_address, offset, words)
    }

    fn write_frame(
        &self,
        physical_address: u64,
        offset: usize,
        words: &[u64],
    ) -> Result<(), MmuError> {
        paging::write_frame(self, physical_address, offset, words)
    }

    fn zero_frame(&self, physical_address: u64) -> Result<(), MmuError> {
        self.with_table(physical_address, PageTable::clear)
    }

    fn read_mapped_frame(
        &self,
        address: usize,
        offset: usize,
        words: &mut [u64],
    ) -> Result<(), MmuError> {
        paging::read_mapped_frame(self, address, offset, words)
    }

    fn write_mapped_frame(
        &self,
        address: usize,
        offset: usize,
        words: &[u64],
    ) -> Result<(), MmuError> {
        paging::write_mapped_frame(self, address, offset, words)
    }

    fn invalidate_page(&self, address: usize) -> Result<(), MmuError> {
        if !self.is_valid_virtual_address(address) {
            return Err(MmuError::InvalidAddress);
        }
        invalidate_address(address);
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
        if !range.is_empty() {
            synchronize_instructions();
        }
        Ok(())
    }

    fn synchronize_frame(&self, frame: Frame) -> Result<(), MmuError> {
        paging::validate_physical::<Self>(frame.address())?;
        synchronize_instructions();
        Ok(())
    }
}

fn synchronize_instructions() {
    // REFERENCE: https://github.com/torvalds/linux/blob/master/arch/loongarch/include/asm/cacheflush.h
    // SAFETY: 此前代码写入须已完成，处理器须允许执行内存与取指同步。
    unsafe { asm!("dbar 0", "ibar 0", options(nostack)) };
}

/// # Safety
///
/// 只能作为物理 refill 入口执行，页表根和遍历参数须有效且持续可读。
#[unsafe(naked)]
#[unsafe(link_section = ".text.entry.refill")]
pub(super) extern "C" fn refill() -> ! {
    naked_asm!(
        "csrwr $t0, 0x8b",
        "csrrd $t0, 0x1b",
        "beqz $t0, 2f",
        "lddir $t0, $t0, 3",
        "beqz $t0, 2f",
        "lddir $t0, $t0, 2",
        "beqz $t0, 2f",
        "lddir $t0, $t0, 1",
        "beqz $t0, 2f",
        "ldpte $t0, 0",
        "ldpte $t0, 1",
        "b 3f",
        "2:",
        // 填入 4 KiB 无效项，重试后由硬件报告 PIL/PIS/PIF。
        "csrwr $zero, 0x8c",
        "csrwr $zero, 0x8d",
        "csrrd $t0, 0x8e",
        "bstrins.d $t0, $zero, 5, 0",
        "ori $t0, $t0, 12",
        "csrwr $t0, 0x8e",
        "3:",
        "tlbfill",
        "csrrd $t0, 0x8b",
        "ertn",
    );
}
