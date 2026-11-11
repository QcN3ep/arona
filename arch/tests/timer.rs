// SPDX-License-Identifier: MPL-2.0

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use arch::device::uart::Uart;
use kernel::{
    hardware::{self, cpu::InterruptState},
    memory::VirtRange,
    trap::{Trap, TrapEvent, TrapOrigin, TrapOutcome},
};

#[cfg(target_arch = "riscv64")]
#[path = "riscv64/timer.rs"]
mod isa;
#[cfg(target_arch = "loongarch64")]
#[path = "loongarch64/timer.rs"]
mod isa;

#[cfg(target_arch = "riscv64")]
const UART: usize = 0xffff_ffc0_0020_0000;
#[cfg(target_arch = "loongarch64")]
const UART: usize = 0xffff_8000_0060_01e0;

const MILLISECOND: u64 = 1_000_000;
static INTERRUPTS: AtomicUsize = AtomicUsize::new(0);
static DEADLINE: AtomicU64 = AtomicU64::new(0);

unsafe extern "C" {
    static _sbss: u8;
    static _ebss: u8;
    static _boot_stack_bottom: u8;
    static _boot_stack_top: u8;
}

/// # Safety
///
/// May only be called by the test entry point when the higher-half mapping and boot stack are valid and local interrupts are disabled; the arguments must be the values delivered by the firmware.
#[unsafe(no_mangle)]
unsafe extern "C" fn __wrap__start_native(_cpu: usize, firmware: usize) -> ! {
    let start = core::ptr::addr_of!(_sbss) as *mut u8;
    let length = core::ptr::addr_of!(_ebss) as usize - start as usize;
    // SAFETY: The range must be exclusive writable BSS not yet used by the boot entry point.
    unsafe { start.write_bytes(0, length) };
    uart().init().unwrap();
    uart().write(format_args!("Timer tests started\n")).unwrap();
    arch::hardware::bind();
    hardware::trap()
        .install(
            VirtRange {
                start: core::ptr::addr_of!(_boot_stack_bottom) as usize,
                end: core::ptr::addr_of!(_boot_stack_top) as usize,
            },
            handle,
        )
        .unwrap();
    let timer = hardware::timer();
    timer.initialize(isa::frequency(firmware)).unwrap();
    assert!(timer.frequency_hz().unwrap() > 0);
    lifecycle();
    uart().write(format_args!("Timer tests passed\n")).unwrap();
    hardware::cpu().halt()
}

fn lifecycle() {
    let timer = hardware::timer();
    let old_deadline = timer.now_ns().unwrap() + 10 * MILLISECOND;
    timer.set_deadline(old_deadline).unwrap();
    arm(timer.now_ns().unwrap() + 2 * MILLISECOND);
    wait_interrupt(1);
    wait_until(old_deadline + MILLISECOND, true);
    assert_eq!(INTERRUPTS.load(Ordering::Relaxed), 1);

    let deadline = timer.now_ns().unwrap() + 2 * MILLISECOND;
    arm(deadline);
    timer.cancel().unwrap();
    wait_until(deadline + MILLISECOND, true);
    assert_eq!(INTERRUPTS.load(Ordering::Relaxed), 1);

    let deadline = timer.now_ns().unwrap() + MILLISECOND;
    arm(deadline);
    wait_until(deadline + MILLISECOND, false);
    timer.cancel().unwrap();
    timer.cancel().unwrap();
    wait_until(timer.now_ns().unwrap() + MILLISECOND, true);
    assert_eq!(INTERRUPTS.load(Ordering::Relaxed), 1);

    arm(timer.now_ns().unwrap());
    wait_until(timer.now_ns().unwrap() + MILLISECOND, false);
    arm(timer.now_ns().unwrap() + 2 * MILLISECOND);
    #[cfg(target_arch = "loongarch64")]
    assert_eq!(
        timer.set_deadline(u64::MAX),
        Err(kernel::hardware::timer::TimerError::DeadlineOutOfRange)
    );
    wait_interrupt(2);

    arm(timer.now_ns().unwrap());
    wait_until(timer.now_ns().unwrap() + MILLISECOND, false);
    assert_eq!(isa::round_trip(), 0);
    assert_eq!(INTERRUPTS.load(Ordering::Relaxed), 3);
    assert_eq!(hardware::cpu().interrupt_state(), InterruptState::Disabled);
    wait_until(timer.now_ns().unwrap() + MILLISECOND, true);
    assert_eq!(INTERRUPTS.load(Ordering::Relaxed), 3);
}

fn arm(deadline: u64) {
    DEADLINE.store(deadline, Ordering::Relaxed);
    hardware::timer().set_deadline(deadline).unwrap();
}

fn wait_interrupt(count: usize) {
    let limit = hardware::timer().now_ns().unwrap() + 100 * MILLISECOND;
    while INTERRUPTS.load(Ordering::Relaxed) < count {
        assert!(
            hardware::timer().now_ns().unwrap() < limit,
            "timer timed out"
        );
        hardware::cpu().idle_until_interrupt();
        assert_eq!(hardware::cpu().interrupt_state(), InterruptState::Disabled);
    }
    assert_eq!(INTERRUPTS.load(Ordering::Relaxed), count);
}

fn wait_until(deadline: u64, interrupts: bool) {
    while hardware::timer().now_ns().unwrap() < deadline {
        if interrupts {
            hardware::cpu().restore_interrupts(InterruptState::Enabled);
        }
        core::hint::spin_loop();
        hardware::cpu().disable_interrupts();
    }
}

fn handle(trap: Trap) -> TrapOutcome {
    assert_eq!(hardware::cpu().interrupt_state(), InterruptState::Disabled);
    assert_eq!(trap.origin, TrapOrigin::Kernel);
    assert_eq!(trap.event, TrapEvent::TimerInterrupt);
    let timer = hardware::timer();
    assert!(timer.now_ns().unwrap() >= DEADLINE.load(Ordering::Relaxed));
    timer.acknowledge().unwrap();
    INTERRUPTS.fetch_add(1, Ordering::Relaxed);
    TrapOutcome::Resume
}

fn uart() -> Uart {
    // SAFETY: UART must be the boot mapping of the current ISA, and the test must have exclusive access.
    unsafe { Uart::new(UART) }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    let _ = uart().emergency_write(format_args!("Timer tests failed: {info}\n"));
    loop {
        core::hint::spin_loop();
    }
}
