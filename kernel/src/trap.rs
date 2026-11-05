// SPDX-License-Identifier: MPL-2.0

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
    /// 仅用于 Syscall；arch 写入返回寄存器并恢复至 successor。
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
    // TODO: 接入缺页处理。
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
