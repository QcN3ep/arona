// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Temporary boot page tables, higher-half access layout, and paging switch.

use crate::paging::PAGE_TABLE_WINDOW;

pub(super) const LARGE_PAGE_SIZE: usize = 0x20_0000;
const KERNEL_BASE: usize = 0xffff_ffc0_0000_0000;
pub(super) const UART_BASE: usize = KERNEL_BASE + 0x20_0000;
pub(super) const UART_PHYSICAL_BASE: usize = 0x1000_0000;
pub(super) const FIRMWARE_BASE: usize = KERNEL_BASE + 0x40_0000;

// REFERENCE: https://docs.riscv.org/reference/isa/priv/supervisor.html
const VALID: usize = 1;
const READ: usize = 1 << 1;
const WRITE: usize = 1 << 2;
const EXECUTE: usize = 1 << 3;
const ACCESSED: usize = 1 << 6;
const DIRTY: usize = 1 << 7;
const PAGE_RO: usize = VALID | READ | ACCESSED;
const PAGE_RX: usize = PAGE_RO | EXECUTE;
const PAGE_RW: usize = PAGE_RO | WRITE | DIRTY;
const PAGE_RWX: usize = PAGE_RW | EXECUTE;

#[repr(C, align(4096))]
struct PageTable {
    entries: [usize; 512],
}

#[repr(C)]
struct PageTables {
    root: PageTable,
    kernel_l1: PageTable,
    window_l1: PageTable,
}

#[used]
#[unsafe(link_section = ".boot_tables.root")]
static mut TABLES: PageTables = PageTables {
    root: PageTable::new(),
    kernel_l1: PageTable::new(),
    window_l1: PageTable::new(),
};

unsafe extern "C" {
    static mut _boot_tables_phys: PageTables;
    static _boot_tables_window_phys: u8;
    static _kernel_phys_start: u8;
}

/// # Safety
///
/// The processor must run with interrupts disabled at a physical address; the page tables and boot stack must be exclusively readable and writable; dtb must be the physical address delivered by the firmware.
#[unsafe(link_section = ".text.entry.rust")]
pub(super) unsafe extern "C" fn enter(hartid: usize, dtb: usize) -> ! {
    let start = core::ptr::addr_of!(_kernel_phys_start) as usize;

    // SAFETY: The page table region defined by the linker script must be page-aligned and exclusively writable.
    let tables = unsafe { &mut *core::ptr::addr_of_mut!(_boot_tables_phys) };
    tables.init(start, dtb);

    let satp = (8usize << 60) | (&raw const tables.root as usize >> 12);
    // SAFETY: The page tables must cover the current instruction and the higher-half entry, and a0/a1 must preserve the firmware boot arguments.
    unsafe {
        core::arch::asm!(
            "lla t1, 2f",
            "ld t1, 0(t1)",
            "sfence.vma",
            "csrw satp, t0",
            "sfence.vma",
            "jr t1",
            ".balign 8",
            "2:",
            ".dword {virtual_entry}",
            virtual_entry = sym super::_start_virtual,
            in("t0") satp,
            in("a0") hartid,
            in("a1") dtb,
            options(noreturn),
        );
    }
}

impl PageTables {
    #[inline(always)]
    fn init(&mut self, kernel: usize, dtb: usize) {
        self.root.clear();
        self.kernel_l1.clear();
        self.window_l1.clear();

        self.root.link::<2>(KERNEL_BASE, &self.kernel_l1);
        self.root.map::<2>(kernel, kernel & !0x3fff_ffff, PAGE_RX);
        self.root.link::<2>(PAGE_TABLE_WINDOW, &self.window_l1);
        let window_table = core::ptr::addr_of!(_boot_tables_window_phys) as usize;
        // SAFETY: Before paging is enabled, the page table region can be written exclusively through physical addresses.
        unsafe { (&mut *(window_table as *mut PageTable)).clear() };
        self.window_l1
            .link_address::<1>(PAGE_TABLE_WINDOW, window_table);
        // The boot image is temporarily mapped with a single read-write-execute 2 MiB large page.
        self.kernel_l1.map::<1>(KERNEL_BASE, kernel, PAGE_RWX);
        self.kernel_l1
            .map::<1>(UART_BASE, UART_PHYSICAL_BASE, PAGE_RW);

        let dtb_page = dtb & !(LARGE_PAGE_SIZE - 1);
        self.kernel_l1.map::<1>(FIRMWARE_BASE, dtb_page, PAGE_RO);
        self.kernel_l1.map::<1>(
            FIRMWARE_BASE + LARGE_PAGE_SIZE,
            dtb_page.wrapping_add(LARGE_PAGE_SIZE),
            PAGE_RO,
        );
    }
}

impl PageTable {
    const fn new() -> Self {
        Self { entries: [0; 512] }
    }

    #[inline(always)]
    fn clear(&mut self) {
        let mut index = 0usize;
        while index < self.entries.len() {
            self.write(index, 0);
            index = index.wrapping_add(1);
        }
    }

    #[inline(always)]
    fn link<const LEVEL: usize>(&mut self, virtual_address: usize, next: &Self) {
        let physical = next as *const Self as usize;
        self.write(virtual_address >> (12 + 9 * LEVEL), (physical >> 2) | VALID);
    }

    #[inline(always)]
    fn link_address<const LEVEL: usize>(&mut self, virtual_address: usize, physical: usize) {
        self.write(virtual_address >> (12 + 9 * LEVEL), (physical >> 2) | VALID);
    }

    #[inline(always)]
    fn map<const LEVEL: usize>(&mut self, virtual_address: usize, physical: usize, flags: usize) {
        self.write(virtual_address >> (12 + 9 * LEVEL), (physical >> 2) | flags);
    }

    #[inline(always)]
    fn write(&mut self, index: usize, value: usize) {
        let entry = self.entries.as_mut_ptr().wrapping_add(index & 511);
        // SAFETY: entry lies within the exclusive page table; the inline assembly store avoids calling high-address core debug checks before paging is enabled.
        unsafe {
            core::arch::asm!(
                "sd {value}, 0({entry})",
                entry = in(reg) entry,
                value = in(reg) value,
                options(nostack),
            );
        }
    }
}
