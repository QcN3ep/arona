// SPDX-License-Identifier: MPL-2.0

//! Trap handling and exit for user Threads.

use super::{Thread, ThreadError, ThreadState, prepare_wait, scheduler};
use crate::{
    hardware::{cpu::InterruptGuard, execution::UserState},
    trap::{
        Trap, TrapEvent, TrapOutcome,
        port::{ForwardError, TrapContinuation, TrapPortReference},
    },
};
use abi::trap::TrapInfo;

const FAULT_STATUS: usize = usize::MAX;

enum UserAction {
    Return(TrapOutcome),
    Exit(usize),
}

pub(crate) fn handle_trap(trap: Trap) -> TrapOutcome {
    // Exit does not restore the current stack; let the handler's local resources drop normally first.
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
    let port = match thread.task().unwrap().forwarding_port() {
        Ok(port) => port,
        Err(_) => return UserAction::Exit(FAULT_STATUS),
    };
    if let Some(port) = &port
        && let Some(info) = forwarded_info(trap)
    {
        return forward(&thread, port, info);
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
        TrapEvent::ExternalInterrupt => crate::trap::external_interrupt(),
        TrapEvent::SoftwareInterrupt | TrapEvent::OtherInterrupt => {
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
        if let Some(port) = &port {
            let interrupted = core::mem::take(&mut thread.0.data.lock().interrupted);
            if interrupted {
                return forward(
                    &thread,
                    port,
                    TrapInfo {
                        thread: thread.id(),
                        kind: TrapInfo::INTERRUPTED,
                        pc: trap.pc as u64,
                        successor: trap.pc as u64,
                        detail: 0,
                        address: 0,
                    },
                );
            }
        }
    }
    UserAction::Return(outcome)
}

fn forward(thread: &Thread, port: &TrapPortReference, info: TrapInfo) -> UserAction {
    match thread.forward(port, info) {
        Ok(()) => UserAction::Return(TrapOutcome::Resume),
        Err(_) => UserAction::Exit(FAULT_STATUS),
    }
}

fn forwarded_info(trap: Trap) -> Option<TrapInfo> {
    let mut info = TrapInfo {
        thread: super::current()?.id(),
        kind: 0,
        pc: trap.pc as u64,
        successor: trap.pc as u64,
        detail: 0,
        address: 0,
    };
    match trap.event {
        TrapEvent::Syscall { successor, .. } => {
            info.kind = TrapInfo::SYSCALL;
            info.successor = successor as u64;
        }
        TrapEvent::PageFault { address, access } => {
            info.kind = TrapInfo::PAGE_FAULT;
            info.address = address as u64;
            info.detail = access as u64;
        }
        TrapEvent::IllegalInstruction { instruction } => {
            info.kind = TrapInfo::ILLEGAL_INSTRUCTION;
            info.detail = instruction.map_or(0, u64::from);
        }
        TrapEvent::OtherException { class, address } => {
            info.kind = TrapInfo::EXCEPTION;
            info.detail = class as u64;
            info.address = address.unwrap_or(0) as u64;
        }
        _ => return None,
    }
    Some(info)
}

impl Thread {
    pub fn set_user_state(&self, state: &UserState) -> Result<(), ForwardError> {
        let _interrupts = InterruptGuard::new();
        self.task()
            .ok_or(ForwardError::InvalidState)?
            .validate_user_state(state)?;
        let data = self.0.data.lock();
        if data.state != ThreadState::Created {
            return Err(ForwardError::InvalidState);
        }
        data.resources
            .as_ref()
            .ok_or(ForwardError::InvalidState)?
            .context
            .set_user_state(state)?;
        Ok(())
    }

    pub fn interrupt(&self) -> Result<(), ForwardError> {
        let _interrupts = InterruptGuard::new();
        let port = self
            .task()
            .ok_or(ForwardError::InvalidState)?
            .forwarding_port()?
            .ok_or(ForwardError::InvalidState)?;
        let mut data = self.0.data.lock();
        if !matches!(data.state, ThreadState::Ready | ThreadState::Running)
            || data.trapped.is_some()
        {
            return Err(ForwardError::InvalidState);
        }
        if data.state == ThreadState::Running {
            data.interrupted = true;
            return Ok(());
        }
        let generation = data
            .generation
            .checked_add(1)
            .ok_or(ThreadError::GenerationExhausted)?;
        let pc = data
            .resources
            .as_ref()
            .ok_or(ForwardError::InvalidState)?
            .context
            .user_state()?
            .pc;
        drop(data);
        let continuation = TrapContinuation::interrupted(self.clone(), generation, pc)?;
        scheduler::suspend_interrupted(self, generation)?;
        port.publish(continuation)
    }

    pub(crate) fn has_trap(&self, generation: u64) -> bool {
        let data = self.0.data.lock();
        data.trapped == Some(generation)
            && !matches!(data.state, ThreadState::Dying(_) | ThreadState::Dead(_))
    }

    pub(crate) fn trap_state(&self, generation: u64) -> Result<UserState, ForwardError> {
        let data = self.0.data.lock();
        if data.trapped != Some(generation) || data.state != ThreadState::Blocked {
            return Err(ForwardError::InvalidState);
        }
        Ok(data
            .resources
            .as_ref()
            .ok_or(ForwardError::InvalidState)?
            .context
            .user_state()?)
    }

    pub(crate) fn resume_trap(
        &self,
        generation: u64,
        state: &UserState,
    ) -> Result<(), ForwardError> {
        self.task()
            .ok_or(ForwardError::InvalidState)?
            .validate_user_state(state)?;
        let mut data = self.0.data.lock();
        if data.trapped != Some(generation) || data.state != ThreadState::Blocked {
            return Err(ForwardError::InvalidState);
        }
        data.resources
            .as_ref()
            .ok_or(ForwardError::InvalidState)?
            .context
            .set_user_state(state)?;
        data.trapped = None;
        Ok(())
    }

    pub(crate) fn cancel_trap(&self, generation: u64) {
        if self.has_trap(generation) {
            self.0.data.lock().trapped = None;
            scheduler::terminate(self, FAULT_STATUS);
        }
    }

    fn forward(&self, port: &TrapPortReference, info: TrapInfo) -> Result<(), ForwardError> {
        let wait = prepare_wait()?;
        let generation = wait.generation();
        let continuation = TrapContinuation::new(self.clone(), generation, info, wait.waker())?;
        {
            let mut data = self.0.data.lock();
            data.trapped = Some(generation);
            data.interrupted = false;
        }
        port.publish(continuation)?;
        let result = wait.wait();
        self.0.data.lock().trapped = None;
        result.map_err(|error: ThreadError| error.into())
    }
}
