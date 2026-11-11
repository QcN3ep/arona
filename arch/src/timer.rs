// SPDX-License-Identifier: MPL-2.0

//! Local counter state and nanosecond conversion.

use kernel::hardware::{self, cpu::InterruptState, timer::TimerError};
use spin::{Mutex, MutexGuard};

const NANOS_PER_SECOND: u128 = 1_000_000_000;

pub(crate) struct Clock {
    frequency_hz: u64,
    last_ticks: u64,
    elapsed_ticks: u128,
}

impl Clock {
    pub fn new(frequency_hz: u64, ticks: u64) -> Result<Self, TimerError> {
        if frequency_hz == 0 {
            return Err(TimerError::InvalidFrequency);
        }
        Ok(Self {
            frequency_hz,
            last_ticks: ticks,
            elapsed_ticks: 0,
        })
    }

    pub fn frequency_hz(&self) -> u64 {
        self.frequency_hz
    }

    pub fn now_ns(&mut self, ticks: u64) -> u64 {
        self.sample(ticks);
        (self.elapsed_ticks.saturating_mul(NANOS_PER_SECOND) / self.frequency_hz as u128)
            .min(u64::MAX as u128) as u64
    }

    pub fn ticks_until(&mut self, deadline_ns: u64, ticks: u64) -> Result<u64, TimerError> {
        self.sample(ticks);
        let target = (deadline_ns as u128 * self.frequency_hz as u128).div_ceil(NANOS_PER_SECOND);
        u64::try_from(target.saturating_sub(self.elapsed_ticks))
            .map_err(|_| TimerError::DeadlineOutOfRange)
    }

    fn sample(&mut self, ticks: u64) {
        self.elapsed_ticks = self
            .elapsed_ticks
            .saturating_add(ticks.wrapping_sub(self.last_ticks) as u128);
        self.last_ticks = ticks;
    }
}

pub(crate) struct LocalClock {
    clock: Mutex<Option<Clock>>,
}

impl LocalClock {
    pub const fn new() -> Self {
        Self {
            clock: Mutex::new(None),
        }
    }

    pub fn initialize(
        &self,
        create: impl FnOnce() -> Result<Clock, TimerError>,
    ) -> Result<(), TimerError> {
        let mut slot = self.try_lock()?;
        if slot.is_some() {
            return Err(TimerError::AlreadyInitialized);
        }
        *slot = Some(create()?);
        Ok(())
    }

    pub fn with<R>(
        &self,
        operation: impl FnOnce(&mut Clock) -> Result<R, TimerError>,
    ) -> Result<R, TimerError> {
        let mut clock = self.try_lock()?;
        operation(clock.as_mut().ok_or(TimerError::NotInitialized)?)
    }

    fn try_lock(&self) -> Result<MutexGuard<'_, Option<Clock>>, TimerError> {
        check_interrupts()?;
        self.clock.try_lock().ok_or(TimerError::Busy)
    }
}

fn check_interrupts() -> Result<(), TimerError> {
    if hardware::cpu().interrupt_state() != InterruptState::Disabled {
        return Err(TimerError::InterruptsEnabled);
    }
    Ok(())
}
