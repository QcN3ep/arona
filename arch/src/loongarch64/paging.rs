// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use core::arch::{asm, naked_asm};
use core::ops::Range;

use kernel::hardware::mmu::{MemoryType, MmuError, PageAttributes, PageMapping, PagePermissions};

use kernel::memory::Frame;

use crate::paging::{self, Format};

// REFERENCE: https://loongson.github.io/LoongArch-Documentation/LoongArch-Vol1-EN.html
const VALID: u64 = 1;
const DIRTY: u64 = 1 << 1;
const USER: u64 = 3 << 2;
const CACHED: u64 = 1 << 4;
const HUGE: u64 = 1 << 6;
// LDDIR retains only the physical address width. Carry huge leaf access bits
// in software positions until refill reconstructs the TLB entry.
const HUGE_NO_READ: u64 = 1 << 9;
const HUGE_NO_EXECUTE: u64 = 1 << 10;
const NO_READ: u64 = 1 << 61;
const NO_EXECUTE: u64 = 1 << 62;
const PHYSICAL: u64 = ((1 << 48) - 1) & !0xfff;
const PWCL: usize = 12 | (9 << 5) | (21 << 10) | (9 << 15) | (30 << 20) | (9 << 25);
const PWCH: usize = 39 | (9 << 6);

fn invalidate_address(address: usize) {
    // SAFETY: `address` must be a canonical virtual address in the current address space.
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
    static MMU: paging::MmuBackend<LoongArchMmu> = paging::MmuBackend(LoongArchMmu) => kernel::hardware::mmu::bind
);

pub struct LoongArchMmu;

impl Format for LoongArchMmu {
    const LEVELS: usize = 4;
    const VIRTUAL_BITS: u32 = 48;
    const DEVICE_MEMORY_TYPE: MemoryType = MemoryType::Device;
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

    fn encode_mapping(&self, mapping: PageMapping, level: usize) -> Result<u64, MmuError> {
        let attributes = mapping.attributes;
        let permissions = match attributes.permissions {
            PagePermissions::NoAccess => NO_READ | NO_EXECUTE,
            PagePermissions::ReadOnly => NO_EXECUTE,
            PagePermissions::ReadWrite => DIRTY | NO_EXECUTE,
            PagePermissions::ReadExecute => 0,
            PagePermissions::ReadWriteExecute => DIRTY,
            PagePermissions::ExecuteOnly => NO_READ,
        };
        let memory_type = match attributes.memory_type {
            MemoryType::Normal => CACHED,
            MemoryType::Device if permissions & NO_EXECUTE == 0 => {
                return Err(MmuError::InvalidAttributes);
            }
            MemoryType::Device => 0,
        };
        if level > 1 {
            return Err(MmuError::Unsupported);
        }
        let permissions = if level == 1 {
            (permissions & !(NO_READ | NO_EXECUTE)) | ((permissions & (NO_READ | NO_EXECUTE)) >> 52)
        } else {
            permissions
        };
        Ok(mapping.physical_address
            | VALID
            | permissions
            | memory_type
            | if level > 0 { HUGE } else { 0 }
            | if attributes.user_accessible { USER } else { 0 })
    }

    fn decode_mapping(&self, entry: u64, level: usize) -> Result<PageMapping, MmuError> {
        if level > 1
            || entry & VALID == 0
            || entry
                & !(PHYSICAL
                    | VALID
                    | DIRTY
                    | USER
                    | CACHED
                    | HUGE
                    | HUGE_NO_READ
                    | HUGE_NO_EXECUTE
                    | NO_READ
                    | NO_EXECUTE)
                != 0
            || (level > 0) != (entry & HUGE != 0)
        {
            return Err(MmuError::InvalidTable);
        }
        let access = if level == 1 {
            (entry & DIRTY) | ((entry & (HUGE_NO_READ | HUGE_NO_EXECUTE)) << 52)
        } else {
            entry & (DIRTY | NO_READ | NO_EXECUTE)
        };
        let permissions = match access {
            flags if flags == NO_READ | NO_EXECUTE => PagePermissions::NoAccess,
            NO_EXECUTE => PagePermissions::ReadOnly,
            flags if flags == DIRTY | NO_EXECUTE => PagePermissions::ReadWrite,
            0 => PagePermissions::ReadExecute,
            DIRTY => PagePermissions::ReadWriteExecute,
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

    fn is_mapping(entry: u64, level: usize) -> bool {
        level == 0 || entry & HUGE != 0
    }

    fn invalidate_address(&self, address: usize) {
        invalidate_address(address);
    }

    fn switch_root(&self, root: Frame) -> Result<(), MmuError> {
        let _access = paging::lock_memory();
        let root = root.address();
        paging::validate_physical::<Self>(root)?;
        if root == 0 {
            return Err(MmuError::InvalidTable);
        }
        // SAFETY: The new page table must cover the current code, stack, and active data; interrupts must be disabled and the backing pages must remain valid.
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

    fn invalidate_all(&self) {
        // SAFETY: The processor must be allowed to execute local TLB invalidation instructions.
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
    // SAFETY: Prior code writes must have completed, and the processor must be allowed to perform memory and instruction fetch synchronization.
    unsafe { asm!("dbar 0", "ibar 0", options(nostack)) };
}

/// # Safety
///
/// Can only be executed as the physical refill entry; the page table root and walk parameters must be valid and persistently readable.
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
        "csrwr $t1, 0x8c",
        "srli.d $t1, $t0, 9",
        "bstrins.d $t0, $t1, 62, 61",
        "bstrins.d $t0, $zero, 10, 9",
        "csrrd $t1, 0x8c",
        "ldpte $t0, 0",
        "ldpte $t0, 1",
        "b 3f",
        "2:",
        // Fill in a 4 KiB invalid entry; after retry, the hardware reports PIL/PIS/PIF.
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
