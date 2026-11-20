// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use core::arch::asm;
use core::ops::Range;

use kernel::hardware::mmu::{MemoryType, MmuError, PageAttributes, PageMapping, PagePermissions};

use kernel::memory::Frame;

use crate::paging::{self, Format};

// REFERENCE: https://docs.riscv.org/reference/isa/priv/supervisor.html
const VALID: u64 = 1;
const READ: u64 = 1 << 1;
const WRITE: u64 = 1 << 2;
const EXECUTE: u64 = 1 << 3;
const USER: u64 = 1 << 4;
const ACCESSED: u64 = 1 << 6;
const DIRTY: u64 = 1 << 7;
// Dirty marks an invalid leaf that retains its backing page frames. A non-leaf
// entry must keep dirty clear, and hardware ignores dirty while valid is clear, so
// a set dirty bit cannot be mistaken for a table entry above the base level; the
// RSW bits remain available for software-defined metadata.
const NO_ACCESS: u64 = 1 << 7;
const PPN: u64 = ((1 << 44) - 1) << 10;

fn invalidate_address(address: usize) {
    // SAFETY: address must be a canonical virtual address in the current address space.
    unsafe { asm!("sfence.vma {address}, zero", address = in(reg) address, options(nostack)) };
}

crate::declare_hardware!(
    static MMU: paging::MmuBackend<Sv39> = paging::MmuBackend(Sv39) => kernel::hardware::mmu::bind
);

pub struct Sv39;

impl Format for Sv39 {
    const LEVELS: usize = 3;
    const VIRTUAL_BITS: u32 = 39;
    const DEVICE_MEMORY_TYPE: MemoryType = MemoryType::Normal;
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

    fn encode_mapping(&self, mapping: PageMapping, level: usize) -> Result<u64, MmuError> {
        let attributes = mapping.attributes;
        let permissions = match attributes.permissions {
            PagePermissions::NoAccess => NO_ACCESS,
            PagePermissions::ReadOnly => READ,
            PagePermissions::ReadWrite => READ | WRITE | DIRTY,
            PagePermissions::ReadExecute => READ | EXECUTE,
            PagePermissions::ReadWriteExecute => READ | WRITE | EXECUTE | DIRTY,
            PagePermissions::ExecuteOnly => EXECUTE,
        };
        if attributes.memory_type == MemoryType::Device {
            return Err(if permissions & EXECUTE != 0 {
                MmuError::InvalidAttributes
            } else {
                MmuError::Unsupported
            });
        }
        if level > 1 {
            return Err(MmuError::Unsupported);
        }
        Ok((mapping.physical_address >> 2)
            | if attributes.permissions == PagePermissions::NoAccess {
                0
            } else {
                VALID | ACCESSED
            }
            | permissions
            | if attributes.user_accessible { USER } else { 0 })
    }

    fn decode_mapping(&self, entry: u64, level: usize) -> Result<PageMapping, MmuError> {
        if level > 1 {
            return Err(MmuError::InvalidTable);
        }
        let no_access = entry & !(PPN | USER) == NO_ACCESS;
        if !no_access
            && (entry & VALID == 0
                || entry & !(PPN | VALID | READ | WRITE | EXECUTE | USER | ACCESSED | DIRTY) != 0)
        {
            return Err(MmuError::InvalidTable);
        }
        let permissions = if no_access {
            PagePermissions::NoAccess
        } else {
            match entry & (READ | WRITE | EXECUTE) {
                READ => PagePermissions::ReadOnly,
                flags if flags == READ | WRITE => PagePermissions::ReadWrite,
                flags if flags == READ | EXECUTE => PagePermissions::ReadExecute,
                flags if flags == READ | WRITE | EXECUTE => PagePermissions::ReadWriteExecute,
                EXECUTE => PagePermissions::ExecuteOnly,
                _ => return Err(MmuError::InvalidAttributes),
            }
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

    fn is_mapping(entry: u64, level: usize) -> bool {
        level == 0 || entry & (READ | WRITE | EXECUTE | NO_ACCESS) != 0
    }

    fn invalidate_address(&self, address: usize) {
        invalidate_address(address);
    }

    fn switch_root(&self, root: Frame) -> Result<(), MmuError> {
        let _access = paging::lock_memory();
        let root = root.address();
        paging::validate_physical::<Self>(root)?;
        let satp = (8usize << 60) | (root as usize >> 12);
        // SAFETY: The new page table must cover the current code, stack, and active data; interrupts must be disabled and the backing pages must remain valid.
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

    fn invalidate_all(&self) {
        // SAFETY: The processor must allow executing the S-mode address translation synchronization instruction.
        unsafe { asm!("sfence.vma", options(nostack)) };
    }

    fn synchronize_code(&self, range: Range<usize>) -> Result<(), MmuError> {
        paging::validate_code_range(&range, 39)?;
        if !range.is_empty() {
            // SAFETY: The range must be a valid normal memory mapping, and prior writes must have completed.
            unsafe { asm!("fence.i", options(nostack)) };
        }
        Ok(())
    }

    fn synchronize_frame(&self, frame: Frame) -> Result<(), MmuError> {
        paging::validate_physical::<Self>(frame.address())?;
        // SAFETY: Prior page frame writes must have completed, and the processor must allow executing the instruction cache synchronization.
        unsafe { asm!("fence.i", options(nostack)) };
        Ok(())
    }
}
