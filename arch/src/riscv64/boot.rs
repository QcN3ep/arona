// SPDX-License-Identifier: MPL-2.0

use core::mem::size_of;

use kernel::{BootInfo, HardwareInfo, PhysRange};

use crate::boot::dtb::Dtb;

static mut HARDWARE_INFO: HardwareInfo = HardwareInfo::new();

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

    // SAFETY: 可变借用存续期间须独占访问 HARDWARE_INFO。
    let hardware = unsafe { &mut *core::ptr::addr_of_mut!(HARDWARE_INFO) };
    // SAFETY: dtb 须指向解析期间可读且不变的完整 DTB。
    unsafe { Dtb::new(dtb as *const u8) }
        .and_then(|dtb| dtb.dump(hardware))
        .unwrap_or_else(|error| panic!("failed to parse DTB: {error}"));
    let info = BootInfo {
        boot_cpu_id: hartid as u64,
        kernel_image: PhysRange {
            start: core::ptr::addr_of!(_kernel_start) as u64,
            end: core::ptr::addr_of!(_kernel_end) as u64,
        },
        hardware,
    };

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

pub fn halt() -> ! {
    loop {
        // SAFETY: 需要处理器位于 S-mode 且 mstatus.TW = 0
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack, preserves_flags));
        }
    }
}
