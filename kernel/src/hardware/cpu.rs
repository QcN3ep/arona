// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Local interrupt control, processor waiting, and halt capability.

use core::marker::PhantomData;

use super::{BindError, Slot};

static SLOT: Slot<&'static dyn Cpu> = Slot::new();

pub fn bind(cpu: &'static dyn Cpu) -> Result<(), BindError> {
    SLOT.bind(cpu)
}

pub fn cpu() -> &'static dyn Cpu {
    SLOT.get()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterruptState {
    Disabled,
    Enabled,
}

/// # Safety
///
/// Creation and drop must occur on the same processor with local interrupt control permitted; the exception entry and its stack must be ready before interrupts are restored.
#[must_use]
pub(crate) struct InterruptGuard {
    state: InterruptState,
    local: PhantomData<*mut ()>,
}

impl InterruptGuard {
    pub(crate) fn new() -> Self {
        Self {
            state: cpu().disable_interrupts(),
            local: PhantomData,
        }
    }
}

impl Drop for InterruptGuard {
    fn drop(&mut self) {
        cpu().restore_interrupts(self.state);
    }
}

/// Operations affect only the current processor and neither block on locks nor allocate memory.
///
/// # Safety
///
/// The execution environment must permit local interrupt control and wait instructions; the exception entry and its stack must be ready before interrupts are enabled.
/// Local interrupts must already be disabled when idle_until_interrupt is called.
pub trait Cpu: Sync {
    fn interrupt_state(&self) -> InterruptState;

    /// Atomically disables interrupts and returns the previous state; nested calls restore in reverse order.
    fn disable_interrupts(&self) -> InterruptState;

    /// Restores only the local interrupt enable state, preserving other processor state.
    fn restore_interrupts(&self, state: InterruptState);

    /// Waits with interrupts disabled while allowing interrupts to be handled, returning with interrupts still disabled; spurious wakeups are permitted and pending interrupts are not lost.
    fn idle_until_interrupt(&self);

    /// Disables local interrupts and permanently stops the current control flow.
    fn halt(&self) -> !;
}
