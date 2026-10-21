// SPDX-License-Identifier: MPL-2.0

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrapOrigin {
    Kernel,
    User,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FaultAccess {
    Read,
    Write,
    Execute,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExceptionClass {
    MisalignedAddress,
    AccessFault,
    BoundsCheck,
    Breakpoint,
    Arithmetic,
    Privilege,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrapEvent {
    Syscall {
        number: usize,
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
    // TODO: 接入缺页处理、系统调用和中断处理。
    log::error!(
        "Unhandled {:?} trap at {:#x}: {:?}",
        trap.origin,
        trap.pc,
        trap.event
    );
    TrapOutcome::Stop
}
