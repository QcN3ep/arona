// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use alloc::{collections::VecDeque, vec::Vec};
use core::ops::Deref;
use spin::Mutex;

use crate::{
    hardware::{
        cpu::InterruptGuard,
        execution::{ExecutionError, RegisterWrite},
    },
    ipc::wait::{Observer, Observers, READABLE, notify_observers},
    memory::{self, HeapArc, MemoryError},
    task::Task,
    thread::{Thread, ThreadError, Waker},
};
use abi::trap::TrapInfo;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ForwardError {
    Closed,
    WouldBlock,
    InvalidState,
    Memory(MemoryError),
    Execution(ExecutionError),
    Thread(ThreadError),
}

impl From<MemoryError> for ForwardError {
    fn from(error: MemoryError) -> Self {
        Self::Memory(error)
    }
}
impl From<ExecutionError> for ForwardError {
    fn from(error: ExecutionError) -> Self {
        Self::Execution(error)
    }
}
impl From<ThreadError> for ForwardError {
    fn from(error: ThreadError) -> Self {
        Self::Thread(error)
    }
}

pub struct TrapPort {
    reference: TrapPortReference,
}

#[derive(Clone)]
pub struct TrapPortReference(HeapArc<Mutex<PortState>>);

struct PortState {
    credits: usize,
    closed: bool,
    observers: Observers,
    owner: Task,
    bindings: Vec<Task>,
    queue: VecDeque<TrapContinuation>,
    deliveries: usize,
}

pub struct TrapContinuation {
    reference: TrapContinuationReference,
}

#[derive(Clone)]
pub struct TrapContinuationReference(HeapArc<Mutex<ContinuationState>>);

struct ContinuationState {
    thread: Thread,
    generation: u64,
    info: TrapInfo,
    completion: Option<Completion>,
}

enum Completion {
    Waiting(Waker),
    Interrupted,
}

pub struct TrapDelivery {
    port: TrapPortReference,
    continuation: Option<TrapContinuation>,
}

impl TrapPort {
    pub(crate) fn new(owner: Task) -> Result<Self, ForwardError> {
        Ok(Self {
            reference: TrapPortReference(memory::manager().allocate_arc(Mutex::new(
                PortState {
                    credits: 1,
                    closed: false,
                    observers: Observers::new(),
                    owner,
                    bindings: Vec::new(),
                    queue: VecDeque::new(),
                    deliveries: 0,
                },
            ))?),
        })
    }

    pub fn reference(&self) -> TrapPortReference {
        self.reference.clone()
    }
}

impl Clone for TrapPort {
    fn clone(&self) -> Self {
        let _interrupts = InterruptGuard::new();
        self.reference.0.lock().credits += 1;
        Self {
            reference: self.reference(),
        }
    }
}

impl Deref for TrapPort {
    type Target = TrapPortReference;
    fn deref(&self) -> &Self::Target {
        &self.reference
    }
}

impl Drop for TrapPort {
    fn drop(&mut self) {
        let _interrupts = InterruptGuard::new();
        let mut state = self.reference.0.lock();
        state.credits -= 1;
        let close = state.credits == 0;
        drop(state);
        if close {
            self.reference.close();
        }
    }
}

impl TrapPortReference {
    pub(crate) fn same(&self, other: &Self) -> bool {
        HeapArc::ptr_eq(&self.0, &other.0)
    }

    pub(crate) fn bind(&self, task: &Task) -> Result<(), ForwardError> {
        let _interrupts = InterruptGuard::new();
        let mut state = self.0.lock();
        if state.closed || state.owner.same(task) {
            return Err(ForwardError::InvalidState);
        }
        state
            .bindings
            .try_reserve(1)
            .map_err(|_| MemoryError::NoMemory)?;
        state.bindings.push(task.clone());
        Ok(())
    }

    pub(crate) fn readiness(&self) -> Result<u64, ForwardError> {
        let state = self.0.lock();
        if state.closed {
            return Err(ForwardError::Closed);
        }
        Ok(if state.queue.is_empty() { 0 } else { READABLE })
    }

    pub(crate) fn register_observer(&self, observer: Observer) -> Result<(), MemoryError> {
        self.0.lock().observers.insert(observer)
    }

    pub(crate) fn unregister_observer(&self, observer: &Observer) {
        self.0.lock().observers.remove(observer);
    }

    pub(crate) fn identity(&self) -> usize {
        self.0.as_ptr().addr()
    }

    fn notify_observers(&self) {
        notify_observers(|index| self.0.lock().observers.get(index));
    }

    pub(crate) fn unbind(&self, task: &Task) {
        let _interrupts = InterruptGuard::new();
        self.0.lock().bindings.retain(|bound| !bound.same(task));
    }

    pub(crate) fn publish(&self, continuation: TrapContinuation) -> Result<(), ForwardError> {
        let _interrupts = InterruptGuard::new();
        {
            let mut state = self.0.lock();
            if state.closed {
                return Err(ForwardError::Closed);
            }
            let additional = state
                .deliveries
                .checked_add(1)
                .ok_or(MemoryError::NoMemory)?;
            state
                .queue
                .try_reserve(additional)
                .map_err(|_| MemoryError::NoMemory)?;
            state.queue.push_back(continuation);
        }
        self.notify_observers();
        Ok(())
    }

    pub fn try_receive(&self) -> Result<TrapDelivery, ForwardError> {
        let _interrupts = InterruptGuard::new();
        loop {
            let continuation = {
                let mut state = self.0.lock();
                if state.closed {
                    return Err(ForwardError::Closed);
                }
                let continuation = state.queue.pop_front().ok_or(ForwardError::WouldBlock)?;
                state.deliveries += 1;
                continuation
            };
            if continuation.reference.is_pending() {
                return Ok(TrapDelivery {
                    port: self.clone(),
                    continuation: Some(continuation),
                });
            }
            self.0.lock().deliveries -= 1;
            drop(continuation);
        }
    }

    pub(crate) fn close(&self) {
        let _interrupts = InterruptGuard::new();
        let (owner, bindings, queue) = {
            let mut state = self.0.lock();
            if state.closed {
                return;
            }
            state.closed = true;
            (
                state.owner.clone(),
                core::mem::take(&mut state.bindings),
                core::mem::take(&mut state.queue),
            )
        };
        self.notify_observers();
        owner.remove_trap_port(self);
        drop(queue);
        for task in bindings {
            task.terminate(usize::MAX);
        }
    }
}

impl TrapDelivery {
    pub fn info(&self) -> TrapInfo {
        let _interrupts = InterruptGuard::new();
        self.continuation.as_ref().unwrap().reference.0.lock().info
    }
    pub fn commit(mut self) -> TrapContinuation {
        let _interrupts = InterruptGuard::new();
        self.port.0.lock().deliveries -= 1;
        self.continuation.take().unwrap()
    }
}

impl Drop for TrapDelivery {
    fn drop(&mut self) {
        let _interrupts = InterruptGuard::new();
        if let Some(continuation) = self.continuation.take() {
            let mut state = self.port.0.lock();
            state.deliveries -= 1;
            if !state.closed {
                state.queue.push_front(continuation);
                drop(state);
                self.port.notify_observers();
            } else {
                drop(state);
                drop(continuation);
            }
        }
    }
}

impl TrapContinuation {
    pub(crate) fn new(
        thread: Thread,
        generation: u64,
        info: TrapInfo,
        waker: Waker,
    ) -> Result<Self, ForwardError> {
        Ok(Self {
            reference: TrapContinuationReference(memory::manager().allocate_arc(Mutex::new(
                ContinuationState {
                    thread,
                    generation,
                    info,
                    completion: Some(Completion::Waiting(waker)),
                },
            ))?),
        })
    }
    pub fn reference(&self) -> TrapContinuationReference {
        self.reference.clone()
    }

    pub(crate) fn interrupted(
        thread: Thread,
        generation: u64,
        pc: usize,
    ) -> Result<Self, ForwardError> {
        let info = TrapInfo {
            thread: thread.id(),
            kind: TrapInfo::INTERRUPTED,
            pc: pc as u64,
            successor: pc as u64,
            detail: 0,
            address: 0,
        };
        Ok(Self {
            reference: TrapContinuationReference(memory::manager().allocate_arc(Mutex::new(
                ContinuationState {
                    thread,
                    generation,
                    info,
                    completion: Some(Completion::Interrupted),
                },
            ))?),
        })
    }
}

impl TrapContinuationReference {
    pub(crate) fn is_pending(&self) -> bool {
        let state = self.0.lock();
        state.completion.is_some() && state.thread.has_trap(state.generation)
    }

    pub fn register_size(&self, group: u64) -> Result<usize, ForwardError> {
        let _interrupts = InterruptGuard::new();
        let state = self.0.lock();
        if state.completion.is_none() {
            return Err(ForwardError::InvalidState);
        }
        state.thread.trap_register_size(state.generation, group)
    }

    pub fn read_registers(&self, group: u64, output: &mut [u8]) -> Result<(), ForwardError> {
        let _interrupts = InterruptGuard::new();
        let state = self.0.lock();
        if state.completion.is_none() {
            return Err(ForwardError::InvalidState);
        }
        state.thread.trap_registers(state.generation, group, output)
    }

    pub fn resume_registers(&self, groups: &[RegisterWrite<'_>]) -> Result<(), ForwardError> {
        let _interrupts = InterruptGuard::new();
        let mut state = self.0.lock();
        if state.completion.is_none() {
            return Err(ForwardError::InvalidState);
        }
        state.thread.resume_trap(state.generation, groups)?;
        match state.completion.take().unwrap() {
            Completion::Waiting(waker) => {
                waker.wake();
            }
            Completion::Interrupted => crate::thread::scheduler::resume_interrupted(&state.thread),
        }
        Ok(())
    }
}

impl Drop for TrapContinuation {
    fn drop(&mut self) {
        let _interrupts = InterruptGuard::new();
        let mut state = self.reference.0.lock();
        if state.completion.take().is_some() {
            let thread = state.thread.clone();
            let generation = state.generation;
            drop(state);
            thread.cancel_trap(generation);
        }
    }
}
