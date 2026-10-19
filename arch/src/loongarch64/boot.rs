// SPDX-License-Identifier: MPL-2.0

use core::mem::size_of;

unsafe extern "C" {
    static _sbss: u8;
    static _ebss: u8;
}

/// QEMU virt 上 LoongArch 64 直启入口。
///
/// # Safety
///
/// 只能由 QEMU 调用一次。
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "csrwr $zero, 0x4",
        "li.d $t0, 4",
        "csrxchg $zero, $t0, 0x0",
        "la.local $sp, _boot_stack_top",
        "bl {entry}",
        "b .",
        entry = sym _start_native,
    );
}

/// # Safety
///
/// 只能由 [`_start`] 在系统启动时调用一次。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start_native(_argc: usize, _argv: usize, _system_table: usize) -> ! {
    clear_bss();
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
        unsafe {
            core::arch::asm!("idle 0", options(nomem, nostack));
        }
    }
}
