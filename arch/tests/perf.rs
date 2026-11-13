// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#![no_std]
#![no_main]

use arch::device::uart::Uart;

#[cfg(target_arch = "riscv64")]
#[path = "riscv64/perf.rs"]
mod isa;
#[cfg(target_arch = "loongarch64")]
#[path = "loongarch64/perf.rs"]
mod isa;

#[cfg(target_arch = "riscv64")]
const UART: usize = 0xffff_ffc0_0020_0000;
#[cfg(target_arch = "loongarch64")]
const UART: usize = 0xffff_8000_0060_01e0;

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
    assert_eq!(isa::empty(), 0);
    assert_eq!(isa::straight(), 4);
    assert_eq!(isa::loops(), 16);
    assert_eq!(isa::call(), 1);
    assert_eq!(isa::retry(), 75);
    assert_eq!(isa::interrupt(), 1);
    assert_eq!(isa::retranslation(), 2);
    uart()
        .write(format_args!("Perf calibration passed\n"))
        .unwrap();
    kernel::hardware::cpu().halt()
}

fn uart() -> Uart {
    // SAFETY: The boot UART mapping is valid and this single-CPU program owns it exclusively.
    unsafe { Uart::new(UART) }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    let _ = uart().emergency_write(format_args!("Perf calibration failed: {info}\n"));
    kernel::hardware::cpu().halt()
}
