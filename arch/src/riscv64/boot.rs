// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use kernel::hardware::cpu::Cpu;
use kernel::hardware::mmu::MemoryType;
use kernel::{
    BootConsoleMapping, HardwareInfo, log, logger,
    memory::{MemoryRegionState, PhysRange},
};

use super::cpu::CPU;
use crate::boot::{console::EarlyConsole, dtb::Dtb};
use crate::device::uart::Uart;

mod paging;

use paging::{FIRMWARE_BASE, LARGE_PAGE_SIZE, UART_BASE, UART_PHYSICAL_BASE};

static mut HARDWARE_INFO: HardwareInfo = HardwareInfo::new();
// SAFETY: The address must correspond to the 16550 UART on QEMU virt with byte width and stride of 1, and normal access must be exclusive to this instance.
pub(super) static EARLY_CONSOLE: EarlyConsole = EarlyConsole::new(unsafe { Uart::new(UART_BASE) });

unsafe extern "C" {}

// REFERENCE: https://docs.riscv.org/reference/isa/priv/supervisor.html

/// # Safety
///
/// Must only be entered by the firmware at a physical address following the ELF entry convention.
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "csrw sie, zero",
        "li t0, 0x6602",
        "csrc sstatus, t0",
        "lla sp, _boot_stack_phys_top",
        "li tp, 0",
        "lla t0, 1f",
        "csrw stvec, t0",
        "tail {enter}",
        ".balign 4",
        "1:",
        "wfi",
        "j 1b",
        enter = sym paging::enter,
    );
}

/// # Safety
///
/// Must only be entered after `_start` has built the page table and switched to the higher half.
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.boot")]
unsafe extern "C" fn _start_virtual(_hartid: usize, _dtb: usize) -> ! {
    core::arch::naked_asm!(
        "lla t0, _boot_trap",
        "csrw stvec, t0",
        "lla t0, _boot_root",
        "sd zero, 16(t0)",
        "sfence.vma",
        "lla sp, _boot_stack_top",
        "tail _start_native",
    );
}

/// # Safety
///
/// Must be called only once by `_start` after the higher-half mapping and stack are established at system boot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start_native(hartid: usize, dtb: usize) -> ! {
    crate::boot::clear_bss();
    EARLY_CONSOLE.init();
    kernel::boot::register_early_console(&EARLY_CONSOLE).expect("failed to register early console");
    logger::init(log::LevelFilter::Debug).expect("failed to initialize logger");
    log::info!(target: "arch::boot", "Booting kernel on CPU {hartid}");

    // SAFETY: HARDWARE_INFO must be accessed exclusively for the lifetime of the mutable borrow.
    let hardware = unsafe { &mut *core::ptr::addr_of_mut!(HARDWARE_INFO) };
    // SAFETY: dtb must point to a complete DTB that remains readable and unchanged during parsing.
    let tree = unsafe { Dtb::new((FIRMWARE_BASE + (dtb & (LARGE_PAGE_SIZE - 1))) as *const u8) }
        .unwrap_or_else(|error| panic!("failed to read DTB: {error}"));
    let dtb_range = PhysRange {
        start: dtb as u64,
        end: (dtb as u64)
            .checked_add(tree.total_size() as u64)
            .expect("DTB address range overflow"),
    };
    let external_initramfs = tree
        .initramfs()
        .unwrap_or_else(|error| panic!("failed to read initramfs description: {error}"));
    tree.dump(hardware)
        .unwrap_or_else(|error| panic!("failed to parse DTB: {error}"));
    hardware
        .memory
        .reserve(dtb_range, MemoryRegionState::BootReserved)
        .unwrap_or_else(|error| panic!("failed to reserve DTB: {error}"));
    log::debug!(target: "arch::boot", "Parsed DTB at {dtb:#x}");
    crate::boot::enter(
        hartid as u64,
        hardware,
        external_initramfs,
        243,
        BootConsoleMapping {
            virtual_address: UART_BASE,
            physical_address: UART_PHYSICAL_BASE as u64,
            memory_type: MemoryType::Normal,
        },
    )
}

/// # Safety
///
/// Must only be called by the processor while the higher-half mapping and exception entry are valid.
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.boot")]
unsafe extern "C" fn _boot_trap() -> ! {
    core::arch::naked_asm!("wfi", "j _boot_trap");
}

pub fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    CPU.disable_interrupts();
    EARLY_CONSOLE.panic(info);
    CPU.halt()
}
