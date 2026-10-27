// SPDX-License-Identifier: MPL-2.0

//! 用户线程的 Trap 处理与退出。

use super::{ThreadState, scheduler};
use crate::trap::{Trap, TrapEvent, TrapOutcome};

const FAULT_STATUS: usize = usize::MAX;

enum UserAction {
    Return(TrapOutcome),
    Exit(usize),
}

pub(crate) fn handle_trap(trap: Trap) -> TrapOutcome {
    // 退出不再恢复当前栈，先让 handler 的局部资源正常析构。
    match handle(trap) {
        UserAction::Return(outcome) => outcome,
        UserAction::Exit(status) => scheduler::exit(status),
    }
}

fn handle(trap: Trap) -> UserAction {
    let Some(thread) = scheduler::current() else {
        return UserAction::Return(TrapOutcome::Stop);
    };
    if thread.task().is_none() {
        return UserAction::Return(TrapOutcome::Stop);
    }
    if let ThreadState::Dying(status) = thread.state() {
        return UserAction::Exit(status);
    }
    let outcome = match trap.event {
        TrapEvent::Syscall {
            handle, arguments, ..
        } => crate::syscall::dispatch(handle, arguments),
        TrapEvent::TimerInterrupt => match scheduler::timer_interrupt() {
            Ok(()) => TrapOutcome::Resume,
            Err(error) => {
                log::error!("Failed to handle timer interrupt: {error:?}");
                TrapOutcome::Stop
            }
        },
        TrapEvent::ExternalInterrupt | TrapEvent::SoftwareInterrupt | TrapEvent::OtherInterrupt => {
            log::error!("Unhandled interrupt at {:#x}: {:?}", trap.pc, trap.event);
            TrapOutcome::Stop
        }
        _ => {
            log::warn!("User exception at {:#x}: {:?}", trap.pc, trap.event);
            return UserAction::Exit(FAULT_STATUS);
        }
    };
    if outcome != TrapOutcome::Stop {
        if let Err(error) = scheduler::preempt_if_requested() {
            log::error!("Failed to preempt user thread: {error:?}");
            return UserAction::Return(TrapOutcome::Stop);
        }
        if let ThreadState::Dying(status) = thread.state() {
            return UserAction::Exit(status);
        }
    }
    UserAction::Return(outcome)
}
