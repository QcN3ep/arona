// SPDX-License-Identifier: MPL-2.0

use core::mem::size_of;

use kernel::{HardwareInfo, MemoryRegionState, PhysRange, log, logger};

use crate::boot::{console::EarlyConsole, dtb::Dtb, prepare_boot_info};
use crate::device::uart::Uart;

static mut HARDWARE_INFO: HardwareInfo = HardwareInfo::new();
// SAFETY: 地址须对应 QEMU virt 的字节宽度与步长均为 1 的 16550 UART，且普通访问须由此实例独占。
static EARLY_CONSOLE: EarlyConsole = EarlyConsole::new(unsafe { Uart::new(0x1000_0000) });

unsafe extern "C" {
    static _sbss: u8;
    static _ebss: u8;
    static _kernel_start: u8;
    static _kernel_end: u8;
}

/// QEMU virt 上 RISC-V 64 固件入口。
///
/// # Safety
///
/// 只能被固件调用一次。
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "csrw sie, zero",
        "csrci sstatus, 2",
        "lla sp, _boot_stack_top",
        "call {entry}",
        "j .",
        entry = sym _start_native,
    );
}

/// # Safety
///
/// 只能由 [`_start`] 在系统启动时调用一次。
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
    let tree = unsafe { Dtb::new(dtb as *const u8) }
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
    let info = prepare_boot_info(
        hartid as u64,
        PhysRange {
            start: core::ptr::addr_of!(_kernel_start) as u64,
            end: core::ptr::addr_of!(_kernel_end) as u64,
        },
        hardware,
    )
    .unwrap_or_else(|error| panic!("failed to prepare boot information: {error}"));

    log::debug!(target: "arch::boot", "Parsed DTB at {dtb:#x}");
    // SAFETY: 启动信息须与实际硬件一致，处理器须处于内核运行所需的执行环境。
    kernel::boot_kernel(info)
}

pub fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    // SAFETY: 调用方须处于允许写入 `sstatus` 的特权级。
    unsafe {
        core::arch::asm!("csrci sstatus, 2", options(nostack));
    }
    EARLY_CONSOLE.panic(info);
    halt()
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

pub fn halt() -> ! {
    loop {
        // SAFETY: 需要处理器位于 S-mode 且 mstatus.TW = 0
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack, preserves_flags));
        }
    }
}
