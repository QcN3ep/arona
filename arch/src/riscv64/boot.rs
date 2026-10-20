// SPDX-License-Identifier: MPL-2.0

use core::mem::size_of;

use kernel::hardware::mmu::MemoryType;
use kernel::{BootConsoleMapping, HardwareInfo, MemoryRegionState, PhysRange, log, logger};

use crate::boot::{console::EarlyConsole, dtb::Dtb, kernel_image, prepare_boot_info};
use crate::device::uart::Uart;

mod paging;

use paging::{FIRMWARE_BASE, LARGE_PAGE_SIZE, UART_BASE, UART_PHYSICAL_BASE};

static mut HARDWARE_INFO: HardwareInfo = HardwareInfo::new();
// SAFETY: 地址须对应 QEMU virt 的字节宽度与步长均为 1 的 16550 UART，且普通访问须由此实例独占。
static EARLY_CONSOLE: EarlyConsole = EarlyConsole::new(unsafe { Uart::new(UART_BASE) });

unsafe extern "C" {
    static _sbss: u8;
    static _ebss: u8;
}

// REFERENCE: https://docs.riscv.org/reference/isa/priv/supervisor.html

/// # Safety
///
/// 只能由固件按 ELF 入口约定在物理地址启动。
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "csrw sie, zero",
        "csrci sstatus, 2",
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
/// 只能在 `_start` 建立页表并切换至高半区后进入。
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
/// 只能由 `_start` 在系统启动时建立高半区映射与栈后调用一次。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start_native(hartid: usize, dtb: usize) -> ! {
    clear_bss();
    EARLY_CONSOLE.init();
    logger::LOGGER
        .add_sink(&EARLY_CONSOLE)
        .expect("failed to register early console");
    logger::init(log::LevelFilter::Debug).expect("failed to initialize logger");
    log::info!(target: "arch::boot", "Booting kernel on CPU {hartid}");

    // SAFETY: 可变借用存续期间须独占访问 HARDWARE_INFO。
    let hardware = unsafe { &mut *core::ptr::addr_of_mut!(HARDWARE_INFO) };
    // SAFETY: dtb 须指向解析期间可读且不变的完整 DTB。
    let tree = unsafe { Dtb::new((FIRMWARE_BASE + (dtb & (LARGE_PAGE_SIZE - 1))) as *const u8) }
        .unwrap_or_else(|error| panic!("failed to read DTB: {error}"));
    let dtb_range = PhysRange {
        start: dtb as u64,
        end: (dtb as u64)
            .checked_add(tree.total_size() as u64)
            .expect("DTB address range overflow"),
    };
    tree.dump(hardware)
        .unwrap_or_else(|error| panic!("failed to parse DTB: {error}"));
    hardware
        .memory
        .reserve(dtb_range, MemoryRegionState::BootReserved)
        .unwrap_or_else(|error| panic!("failed to reserve DTB: {error}"));
    let mut info = prepare_boot_info(hartid as u64, kernel_image(), hardware)
        .unwrap_or_else(|error| panic!("failed to prepare boot information: {error}"));
    info.early_console = Some(BootConsoleMapping {
        virtual_address: UART_BASE,
        physical_address: UART_PHYSICAL_BASE as u64,
        memory_type: MemoryType::Normal,
    });

    log::debug!(target: "arch::boot", "Parsed DTB at {dtb:#x}");
    crate::hardware::bind();
    // SAFETY: 启动信息须与实际硬件一致，处理器须处于内核运行所需的执行环境。
    kernel::boot_kernel(info)
}

fn clear_bss() {
    let mut cursor = core::ptr::addr_of!(_sbss) as usize;
    let end = core::ptr::addr_of!(_ebss) as usize;

    while cursor < end {
        // SAFETY: 需要保证该地址位于可写且 8 字节对齐的 BSS 中。
        unsafe {
            (cursor as *mut u64).write_volatile(0);
        }
        cursor += size_of::<u64>();
    }
}

/// # Safety
///
/// 只能由处理器在高半区映射和异常入口有效时调用。
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.boot")]
unsafe extern "C" fn _boot_trap() -> ! {
    core::arch::naked_asm!("wfi", "j _boot_trap");
}

pub fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    // SAFETY: 调用方须处于允许写入 `sstatus` 的特权级。
    unsafe {
        core::arch::asm!("csrci sstatus, 2", options(nostack));
    }
    EARLY_CONSOLE.panic(info);
    halt()
}

pub fn halt() -> ! {
    loop {
        // SAFETY: 需要处理器位于 S-mode 且 mstatus.TW = 0
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack, preserves_flags));
        }
    }
}
