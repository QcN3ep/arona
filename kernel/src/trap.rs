// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

pub mod port;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrapOrigin {
    Kernel,
    User,
}

pub use abi::trap::{ExceptionClass, FaultAccess};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrapEvent {
    Syscall {
        handle: usize,
        arguments: [usize; 6],
        successor: usize,
    },
    PageFault {
        address: usize,
        access: FaultAccess,
    },
    TimerInterrupt,
    ExternalInterrupt,
    SoftwareInterrupt,
    OtherInterrupt,
    IllegalInstruction {
        instruction: Option<u32>,
    },
    OtherException {
        class: ExceptionClass,
        address: Option<usize>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Trap {
    pub origin: TrapOrigin,
    pub pc: usize,
    pub event: TrapEvent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrapOutcome {
    Resume,
    /// Used only for Syscall; arch writes the return registers and resumes at the successor.
    Return {
        status: usize,
        values: [usize; 2],
    },
    Stop,
}

pub fn dispatch(trap: Trap) -> TrapOutcome {
    if trap.origin == TrapOrigin::User {
        return crate::thread::handle_trap(trap);
    }
    if trap.event == TrapEvent::TimerInterrupt {
        return match crate::thread::scheduler::timer_interrupt() {
            Ok(()) => TrapOutcome::Resume,
            Err(error) => {
                log::error!("Failed to handle timer interrupt: {error:?}");
                TrapOutcome::Stop
            }
        };
    }
    if trap.event == TrapEvent::ExternalInterrupt {
        return external_interrupt();
    }
    // TODO: Hook up page fault handling.
    log::error!(
        "Unhandled {:?} trap at {:#x}: {:?}",
        trap.origin,
        trap.pc,
        trap.event
    );
    TrapOutcome::Stop
}

pub(crate) fn external_interrupt() -> TrapOutcome {
    match crate::device::dispatch_interrupt() {
        Ok(()) => TrapOutcome::Resume,
        Err(error) => {
            log::error!("Failed to dispatch device interrupt: {error:?}");
            TrapOutcome::Stop
        }
    }
}
