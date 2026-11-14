// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(support::run)]
#![reexport_test_harness_main = "test_main"]

#[path = "kperf/memory.rs"]
mod memory;
mod support;

mod calibration {
    use testkit::{kperf, perf::Perf};

    #[kperf]
    fn empty(perf: &mut Perf) {
        run(perf, empty_body);
    }

    #[kperf]
    fn straight(perf: &mut Perf) {
        run(perf, straight_body);
    }

    #[inline(never)]
    fn run(perf: &mut Perf, operation: extern "C" fn()) {
        perf.iter(|| core::hint::black_box(operation)());
    }

    #[unsafe(naked)]
    extern "C" fn empty_body() {
        #[cfg(target_arch = "riscv64")]
        core::arch::naked_asm!("ret");
        #[cfg(target_arch = "loongarch64")]
        core::arch::naked_asm!("jr $ra");
    }

    #[unsafe(naked)]
    extern "C" fn straight_body() {
        #[cfg(target_arch = "riscv64")]
        core::arch::naked_asm!("nop", "nop", "nop", "nop", "ret");
        #[cfg(target_arch = "loongarch64")]
        core::arch::naked_asm!("nop", "nop", "nop", "nop", "jr $ra");
    }
}

unsafe extern "C" {
    static _sbss: u8;
    static _ebss: u8;
}

/// # Safety
///
/// The boot mapping and stack must be valid, with interrupts disabled and BSS exclusively writable.
#[unsafe(no_mangle)]
unsafe extern "C" fn __wrap__start_native() -> ! {
    let start = core::ptr::addr_of!(_sbss) as *mut u8;
    let length = core::ptr::addr_of!(_ebss) as usize - start as usize;
    // SAFETY: BSS is exclusively writable and has not been used since boot.
    unsafe { start.write_bytes(0, length) };
    uart().init().unwrap();
    arch::hardware::bind();
    test_main();
    kernel::hardware::cpu().halt()
}

fn uart() -> arch::device::uart::Uart {
    #[cfg(target_arch = "riscv64")]
    const UART: usize = 0xffff_ffc0_0020_0000;
    #[cfg(target_arch = "loongarch64")]
    const UART: usize = 0xffff_8000_0060_01e0;
    // SAFETY: The boot UART mapping is valid and this single-CPU program owns it exclusively.
    unsafe { arch::device::uart::Uart::new(UART) }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    let _ = uart().emergency_write(format_args!("KPERF failed: {info}\n"));
    kernel::hardware::cpu().halt()
}
