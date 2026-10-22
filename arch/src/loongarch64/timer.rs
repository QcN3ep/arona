// SPDX-License-Identifier: MPL-2.0

//! LoongArch 稳定计数器与本地定时器。

use core::arch::asm;

use kernel::hardware::timer::{Timer, TimerError};

use crate::timer::{Clock, LocalClock};

// REFERENCE: https://loongson.github.io/LoongArch-Documentation/LoongArch-Vol1-EN.html
const TIMER_INTERRUPT: usize = 1 << 11;

crate::declare_hardware!(
    static TIMER: LoongArchTimer = LoongArchTimer { clock: LocalClock::new() }
        => kernel::hardware::timer::bind
);

struct LoongArchTimer {
    clock: LocalClock,
}

impl Timer for LoongArchTimer {
    fn initialize(&self, frequency_hz: Option<u64>) -> Result<(), TimerError> {
        self.clock.initialize(|| {
            if cpucfg(2) & (1 << 14) == 0 {
                return Err(TimerError::Unsupported);
            }
            let ratio = cpucfg(5);
            let numerator = cpucfg(4) as u64 * (ratio & 0xffff) as u64;
            let divisor = (ratio >> 16) as u64;
            if numerator == 0 || divisor == 0 || !numerator.is_multiple_of(divisor) {
                return Err(TimerError::InvalidFrequency);
            }
            let frequency = numerator / divisor;
            if frequency_hz.is_some_and(|value| value != frequency) {
                return Err(TimerError::InvalidFrequency);
            }
            max_interval()?;
            disarm();
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
            let delay = clock.ticks_until(deadline_ns, counter())?;
            let delay = delay
                .max(4)
                .checked_add(3)
                .ok_or(TimerError::DeadlineOutOfRange)?
                & !3;
            if delay > max_interval()? {
                return Err(TimerError::DeadlineOutOfRange);
            }
            // SAFETY: 当前处理器须独占本地定时器与 ECFG 中的定时中断位。
            unsafe {
                asm!(
                    "csrwr $zero, 0x41",
                    "csrwr {clear}, 0x44",
                    "csrwr {config}, 0x41",
                    "csrxchg {enable}, {mask}, 0x4",
                    clear = inout(reg) 1usize => _,
                    config = inout(reg) delay as usize | 1 => _,
                    enable = inout(reg) TIMER_INTERRUPT => _,
                    mask = in(reg) TIMER_INTERRUPT,
                    options(nostack),
                );
            }
            Ok(())
        })
    }

    fn cancel(&self) -> Result<(), TimerError> {
        self.clock.with(|_| {
            disarm();
            Ok(())
        })
    }

    fn acknowledge(&self) -> Result<(), TimerError> {
        self.cancel()
    }
}

fn cpucfg(index: usize) -> u32 {
    let value: usize;
    // SAFETY: 执行环境须允许读取 CPUCFG。
    unsafe { asm!("cpucfg {}, {}", out(reg) value, in(reg) index, options(nomem, nostack)) };
    value as u32
}

fn counter() -> u64 {
    let ticks: u64;
    // SAFETY: 执行环境须允许读取稳定计数器，CNTC 须保持不变。
    unsafe { asm!("rdtime.d {}, $zero", out(reg) ticks, options(nomem, nostack)) };
    ticks
}

fn max_interval() -> Result<u64, TimerError> {
    let config: usize;
    // SAFETY: 执行环境须允许读取 PRCFG1。
    unsafe { asm!("csrrd {}, 0x21", out(reg) config, options(nomem, nostack)) };
    let bits = ((config >> 4) & 0xff) + 1;
    if !(3..=64).contains(&bits) {
        return Err(TimerError::Unsupported);
    }
    Ok((u64::MAX >> (64 - bits)) & !3)
}

fn disarm() {
    // SAFETY: 当前处理器须独占本地定时器，访问期间须关闭本地中断。
    unsafe {
        asm!(
            "csrxchg $zero, {mask}, 0x4",
            "csrwr $zero, 0x41",
            "csrwr {clear}, 0x44",
            mask = in(reg) TIMER_INTERRUPT,
            clear = inout(reg) 1usize => _,
            options(nostack),
        );
    }
}
