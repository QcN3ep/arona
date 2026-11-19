// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Trap handling and exit for user Threads.

use super::{Thread, ThreadError, ThreadState, prepare_wait, scheduler};
use crate::{
    hardware::{self, cpu::InterruptGuard, execution::RegisterWrite},
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
    pub fn register_size(&self, group: u64) -> Result<usize, ForwardError> {
        let _interrupts = InterruptGuard::new();
        let data = self.0.data.lock();
        if self.task().is_none() || data.resources.is_none() && data.state != ThreadState::Running {
            return Err(ForwardError::InvalidState);
        }
        Ok(hardware::execution().register_size(group)?)
    }

    pub fn read_registers(&self, group: u64, output: &mut [u8]) -> Result<(), ForwardError> {
        let _interrupts = InterruptGuard::new();
        let data = self.0.data.lock();
        if data.state != ThreadState::Created {
            return Err(ForwardError::InvalidState);
        }
        Ok(data
            .resources
            .as_ref()
            .ok_or(ForwardError::InvalidState)?
            .context
            .read_registers(group, output)?)
    }

    pub fn set_registers(&self, groups: &[RegisterWrite<'_>]) -> Result<(), ForwardError> {
        let _interrupts = InterruptGuard::new();
        let space = self
            .task()
            .ok_or(ForwardError::InvalidState)?
            .address_space()
            .map_err(|_| ForwardError::InvalidState)?;
        let data = self.0.data.lock();
        if data.state != ThreadState::Created {
            return Err(ForwardError::InvalidState);
        }
        data.resources
            .as_ref()
            .ok_or(ForwardError::InvalidState)?
            .context
            .set_registers(groups, &mut |entry| space.validate_user_entry(entry))?;
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
            .user_entry()?
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

    pub(crate) fn trap_register_size(
        &self,
        generation: u64,
        group: u64,
    ) -> Result<usize, ForwardError> {
        let data = self.0.data.lock();
        if data.trapped != Some(generation)
            || data.state != ThreadState::Blocked
            || data.resources.is_none()
        {
            return Err(ForwardError::InvalidState);
        }
        Ok(hardware::execution().register_size(group)?)
    }

    pub(crate) fn trap_registers(
        &self,
        generation: u64,
        group: u64,
        output: &mut [u8],
    ) -> Result<(), ForwardError> {
        let data = self.0.data.lock();
        if data.trapped != Some(generation) || data.state != ThreadState::Blocked {
            return Err(ForwardError::InvalidState);
        }
        Ok(data
            .resources
            .as_ref()
            .ok_or(ForwardError::InvalidState)?
            .context
            .read_registers(group, output)?)
    }

    pub(crate) fn resume_trap(
        &self,
        generation: u64,
        groups: &[RegisterWrite<'_>],
    ) -> Result<(), ForwardError> {
        let mut data = self.0.data.lock();
        if data.trapped != Some(generation) || data.state != ThreadState::Blocked {
            return Err(ForwardError::InvalidState);
        }
        // A continuation can outlive its user stack or TLS mapping. The
        // architecture validates the saved registers before restoring them.
        data.resources
            .as_ref()
            .ok_or(ForwardError::InvalidState)?
            .context
            .set_registers(groups, &mut |_| Ok(()))?;
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
