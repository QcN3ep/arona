// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use crate::{
    memory::VirtRange,
    trap::{Trap, TrapOutcome},
};

use super::{BindError, Slot};

static SLOT: Slot<&'static dyn TrapControl> = Slot::new();

pub type Handler = fn(Trap) -> TrapOutcome;

pub fn bind(trap: &'static dyn TrapControl) -> Result<(), BindError> {
    SLOT.bind(trap)
}

pub fn trap() -> &'static dyn TrapControl {
    SLOT.get()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrapError {
    AlreadyInstalled,
    NotInstalled,
    InvalidStack,
    Busy,
    InterruptsEnabled,
}

/// # Safety
///
/// Installation and subsequent operations must run on the same processor; the stack must be exclusive, resident, and fully mapped as kernel-writable memory until replaced or halted.
/// The handler must execute in bounded time without unwinding the stack or enabling interrupts; context switches must go through Execution.
pub trait TrapControl: Sync {
    fn install(&self, stack: VirtRange, handler: Handler) -> Result<(), TrapError>;

    fn set_stack(&self, stack: VirtRange) -> Result<(), TrapError>;
}
