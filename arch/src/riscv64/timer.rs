// SPDX-License-Identifier: MPL-2.0

//! RISC-V time counter and SBI TIME timer.

use core::arch::asm;

use kernel::hardware::timer::{Timer, TimerError};

use crate::timer::{Clock, LocalClock};

// REFERENCE: https://github.com/riscv-non-isa/riscv-sbi-doc/blob/v2.0/src/ext-time.adoc
const SBI_TIME: usize = 0x5449_4d45;

crate::declare_hardware!(
    static TIMER: RiscvTimer = RiscvTimer { clock: LocalClock::new() }
        => kernel::hardware::timer::bind
);

struct RiscvTimer {
    clock: LocalClock,
}

impl Timer for RiscvTimer {
    fn initialize(&self, frequency_hz: Option<u64>) -> Result<(), TimerError> {
        self.clock.initialize(|| {
            let frequency = frequency_hz.ok_or(TimerError::InvalidFrequency)?;
            if frequency == 0 {
                return Err(TimerError::InvalidFrequency);
            }
            if sbi_call(0x10, 3, SBI_TIME)? == 0 {
                return Err(TimerError::Unsupported);
            }
            disarm()?;
            Clock::new(frequency, counter())
        })
    }

    fn frequency_hz(&self) -> Result<u64, TimerError> {
        self.clock.with(|clock| Ok(clock.frequency_hz()))
    }

    fn now_ns(&self) -> Result<u64, TimerError> {
        self.clock.with(|clock| Ok(clock.now_ns(counter())))
    }

    fn set_deadline(&self, deadline_ns: u64) -> Result<(), TimerError> {
        self.clock.with(|clock| {
            let now = counter();
            let delay = clock.ticks_until(deadline_ns, now)?;
            let deadline = now
                .checked_add(delay)
                .ok_or(TimerError::DeadlineOutOfRange)?;
            sbi_call(SBI_TIME, 0, deadline as usize)?;
            // SAFETY: The current processor must exclusively own the supervisor timer interrupt, and the exception entry must be ready.
            unsafe { asm!("csrs sie, {}", in(reg) 1usize << 5, options(nostack)) };
            Ok(())
        })
    }

    fn cancel(&self) -> Result<(), TimerError> {
        self.clock.with(|_| disarm())
    }

    fn acknowledge(&self) -> Result<(), TimerError> {
        self.cancel()
    }
}

fn counter() -> u64 {
    let ticks: u64;
    // SAFETY: The firmware must allow the current processor to read the time counter.
    unsafe { asm!("rdtime {}", out(reg) ticks, options(nomem, nostack)) };
    ticks
}

fn disarm() -> Result<(), TimerError> {
    sbi_call(SBI_TIME, 0, usize::MAX)?;
    // SAFETY: The current processor must exclusively own the supervisor timer interrupt.
    unsafe { asm!("csrc sie, {}", in(reg) 1usize << 5, options(nostack)) };
    Ok(())
}

fn sbi_call(extension: usize, function: usize, argument: usize) -> Result<usize, TimerError> {
    let error: isize;
    let value: usize;
    // SAFETY: The firmware must provide a standard SBI call environment, and the arguments must follow the calling convention of the specified extension.
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") argument => error,
            lateout("a1") value,
            in("a6") function,
            in("a7") extension,
            options(nostack),
        );
    }
    if error != 0 {
        return Err(TimerError::Firmware(error));
    }
    Ok(value)
}
