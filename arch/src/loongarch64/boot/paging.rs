// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Temporary boot page tables, processor mapping configuration, and higher-half switchover.

use crate::{loongarch64::paging::refill, paging::PAGE_TABLE_WINDOW};

const KERNEL_BASE: usize = 0xffff_8000_0020_0000;
pub(super) const MMIO_BASE: usize = 0x8000_0000_0000_0000;
pub(super) const FIRMWARE_BASE: usize = 0x9000_0000_0000_0000;
pub(super) const UART_BASE: usize = KERNEL_BASE + 0x40_0000;
pub(super) const UART_PHYSICAL_BASE: usize = 0x1fe0_0000;

// REFERENCE: https://loongson.github.io/LoongArch-Documentation/LoongArch-Vol1-EN.html
const VALID: usize = 1;
const DIRTY: usize = 1 << 1;
const CACHED: usize = 1 << 4;
const HUGE: usize = 1 << 6;
const NO_EXECUTE: usize = 1 << 62;

#[repr(C, align(4096))]
struct PageTable {
    entries: [usize; 512],
}

#[repr(C)]
struct PageTables {
    root: PageTable,
    kernel_l2: PageTable,
    kernel_l1: PageTable,
    window_l2: PageTable,
    window_l1: PageTable,
}

#[used]
#[unsafe(link_section = ".boot_tables.root")]
static mut TABLES: PageTables = PageTables {
    root: PageTable::new(),
    kernel_l2: PageTable::new(),
    kernel_l1: PageTable::new(),
    window_l2: PageTable::new(),
    window_l1: PageTable::new(),
};

unsafe extern "C" {
    static mut _boot_tables_phys: PageTables;
    static _boot_tables_window_phys: u8;
    static _kernel_phys_start: u8;
    static _kernel_phys_end: u8;
}

/// # Safety
///
/// The processor must have interrupts disabled and be executing at a physical address; the page tables and boot stack must be exclusively readable and writable.
#[unsafe(link_section = ".text.entry.rust")]
pub(super) unsafe extern "C" fn enter(system_table: usize) -> ! {
    let start = core::ptr::addr_of!(_kernel_phys_start) as usize;

    // SAFETY: The page table region defined by the linker script must be page-aligned and exclusively writable.
    let tables = unsafe { &mut *core::ptr::addr_of_mut!(_boot_tables_phys) };
    tables.init(start);

    let entry: usize;
    let trap: usize;
    // SAFETY: The target symbol must be the higher-half entry provided by the linker script.
    unsafe {
        core::arch::asm!(
            "la.abs {entry}, {virtual_entry}",
            "la.abs {trap}, {trap_entry}",
            entry = out(reg) entry,
            trap = out(reg) trap,
            virtual_entry = sym super::_start_virtual,
            trap_entry = sym super::_boot_trap,
            options(nomem, nostack),
        );
    }

    // SAFETY: The processor must have interrupts disabled, and the page tables and physical refill entry must be available.
    unsafe {
        write_csr::<0x1a>(&raw const tables.root as usize); // PGDH
        write_csr::<0x19>(0); // PGDL
        write_csr::<0x18>(0); // ASID
        write_csr::<0x1c>(12 | (9 << 5) | (21 << 10) | (9 << 15) | (30 << 20) | (9 << 25)); // PWCL
        write_csr::<0x1d>(39 | (9 << 6)); // PWCH
        write_csr::<0x1e>(12); // STLBPS
        write_csr::<0x8e>(12); // TLBREHI.PS
        write_csr::<0x88>(refill as *const () as usize); // TLBRENTRY
        write_csr::<0xc>(trap); // EENTRY
        write_csr::<0x180>(0x11); // Low-address DMW keeps instruction fetch working during the switchover.
        write_csr::<0x181>(MMIO_BASE | 1);
        write_csr::<0x182>(FIRMWARE_BASE | 0x11);
        write_csr::<0x183>(0);

        core::arch::asm!(
            "invtlb 0, $zero, $zero",
            "dbar 0",
            "csrwr $t0, 0x0",
            "jirl $zero, $t1, 0",
            in("$t0") 0xb0usize, // PG=1, DA=0, IE=0, cache attribute is CC.
            in("$t1") entry,
            in("$a0") system_table,
            options(noreturn),
        );
    }
}

impl PageTables {
    #[inline(always)]
    fn init(&mut self, kernel: usize) {
        self.root.clear();
        self.kernel_l2.clear();
        self.kernel_l1.clear();
        self.window_l2.clear();
        self.window_l1.clear();

        self.root.link::<3>(KERNEL_BASE, &self.kernel_l2);
        self.kernel_l2.link::<2>(KERNEL_BASE, &self.kernel_l1);
        self.root.link::<3>(PAGE_TABLE_WINDOW, &self.window_l2);
        self.window_l2.link::<2>(PAGE_TABLE_WINDOW, &self.window_l1);
        let window_table = core::ptr::addr_of!(_boot_tables_window_phys) as usize;
        // SAFETY: Before paging is enabled, the page table region can be exclusively written via physical addresses.
        unsafe { (*(window_table as *mut PageTable)).clear() };
        self.window_l1
            .link_address::<1>(PAGE_TABLE_WINDOW, window_table);
        let end = core::ptr::addr_of!(_kernel_phys_end) as usize;
        let mut physical = kernel;
        let mut virtual_address = KERNEL_BASE;
        while physical < end {
            self.kernel_l1
                .map(virtual_address, physical, VALID | DIRTY | CACHED | HUGE);
            physical = physical.wrapping_add(0x20_0000);
            virtual_address = virtual_address.wrapping_add(0x20_0000);
        }
        self.kernel_l1.map(
            UART_BASE,
            UART_PHYSICAL_BASE,
            VALID | DIRTY | HUGE | NO_EXECUTE,
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
        self.write(
            virtual_address >> (12 + 9 * LEVEL),
            next as *const Self as usize,
        );
    }

    #[inline(always)]
    fn link_address<const LEVEL: usize>(&mut self, virtual_address: usize, physical: usize) {
        self.write(virtual_address >> (12 + 9 * LEVEL), physical);
    }

    #[inline(always)]
    fn map(&mut self, virtual_address: usize, physical: usize, flags: usize) {
        self.write(virtual_address >> 21, physical | flags);
    }

    #[inline(always)]
    fn write(&mut self, index: usize, value: usize) {
        let entry = self.entries.as_mut_ptr().wrapping_add(index & 511);
        // SAFETY: The entry lies in an exclusively owned page table; the inline assembly store avoids calling high-address core debug checks before paging is enabled.
        unsafe {
            core::arch::asm!(
                "st.d {value}, {entry}, 0",
                entry = in(reg) entry,
                value = in(reg) value,
                options(nostack),
            );
        }
    }
}

/// # Safety
///
/// The processor must be allowed to access the CSR, and `value` must satisfy the register's configuration constraints.
#[inline(always)]
unsafe fn write_csr<const CSR: u32>(value: usize) {
    // SAFETY: The CSR must be writable, and `value` must be valid.
    unsafe {
        core::arch::asm!(
            "csrwr {value}, {csr}",
            value = inout(reg) value => _,
            csr = const CSR,
            options(nostack),
        );
    }
}
