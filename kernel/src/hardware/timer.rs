// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::{BindError, Slot};

static SLOT: Slot<&'static dyn Timer> = Slot::new();

pub fn bind(timer: &'static dyn Timer) -> Result<(), BindError> {
    SLOT.bind(timer)
}

pub fn timer() -> &'static dyn Timer {
    SLOT.get()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimerError {
    NotInitialized,
    AlreadyInitialized,
    InterruptsEnabled,
    Busy,
    InvalidFrequency,
    Unsupported,
    DeadlineOutOfRange,
    Firmware(isize),
}

/// # Safety
///
/// All operations must run on the same processor with interrupts disabled; before setting a deadline, the exception entry and its stack must be ready.
pub trait Timer: Sync {
    /// Time starts at zero after initialization and the timer interrupt stays disabled; the supplied value is used when there is no hardware frequency source.
    fn initialize(&self, frequency_hz: Option<u64>) -> Result<(), TimerError>;

    fn frequency_hz(&self) -> Result<u64, TimerError>;

    /// Returns non-decreasing nanoseconds since initialization, saturating when out of range.
    fn now_ns(&self) -> Result<u64, TimerError>;

    /// Replaces the absolute nanosecond deadline, rounding up; fires as soon as possible when already expired, and keeps the original timer setting on failure.
    fn set_deadline(&self, deadline_ns: u64) -> Result<(), TimerError>;

    /// Idempotently disables the timer interrupt; the old deadline no longer fires after it returns.
    fn cancel(&self) -> Result<(), TimerError>;

    /// Clears the current interrupt and keeps the timer disabled.
    fn acknowledge(&self) -> Result<(), TimerError>;
}
