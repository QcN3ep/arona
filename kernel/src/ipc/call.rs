// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use alloc::vec::Vec;
use core::{
    mem,
    ops::Deref,
    sync::atomic::{AtomicUsize, Ordering},
};
use spin::Mutex;

use super::{
    ChannelError, ReceivedMessage, Reply,
    wait::{Observer, Observers, notify_observers},
};
use crate::{
    hardware::{self, cpu::InterruptGuard},
    memory::{self, HeapArc, MemoryError},
    task::{HandleEntry, HandleTable, ReservedHandles, Task, TaskError},
    thread::{self, WaitOutcome, Waker},
};

/// # Safety
///
/// Operations and drop must retain the kernel mappings required by the memory manager.
pub struct Call {
    reference: CallReference,
}

/// # Safety
///
/// Operations and drop must retain the kernel mappings required by the memory manager.
#[derive(Clone)]
pub struct CallReference(HeapArc<Mutex<CallState>>);

struct CallState {
    destination: Option<ReservedHandles>,
    phase: CallPhase,
    entries: Vec<HandleEntry>,
    deadline: Option<u64>,
    waker: Option<Waker>,
    observers: Observers,
}

enum CallPhase {
    Pending,
    Ready(Result<super::Message, ChannelError>),
    Finished,
}

static CALLS: AtomicUsize = AtomicUsize::new(0);

pub fn active_calls() -> usize {
    CALLS.load(Ordering::Relaxed)
}

impl Call {
    pub(crate) fn new(task: &Task, deadline: Option<u64>) -> Result<(Self, Reply), TaskError> {
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(super::MESSAGE_HANDLES)
            .map_err(|_| MemoryError::NoMemory)?;
        let destination = task.reserve_reply_handles()?;
        CALLS.fetch_add(1, Ordering::Relaxed);
        let reference = CallReference(memory::manager().allocate_arc(Mutex::new(CallState {
            destination: Some(destination),
            phase: CallPhase::Pending,
            entries,
            deadline,
            waker: None,
            observers: Observers::new(),
        }))?);
        let reply = Reply::new(reference.clone());
        Ok((Self { reference }, reply))
    }

    pub fn reference(&self) -> CallReference {
        self.reference.clone()
    }

    pub(crate) fn wait_with(
        &self,
        deliver: impl FnOnce(&ReceivedMessage, &[HandleEntry]) -> Result<(), TaskError>,
    ) -> Result<ReceivedMessage, TaskError> {
        let wait = {
            let _interrupts = InterruptGuard::new();
            thread::prepare_wait().map_err(ChannelError::from)?
        };
        let deadline = {
            let _interrupts = InterruptGuard::new();
            let mut state = self.reference.0.lock();
            if matches!(state.phase, CallPhase::Pending) {
                state.waker = Some(wait.waker());
            } else {
                drop(state);
                drop(wait);
                return self.receive_deliver(deliver);
            }
            state.deadline
        };
        let outcome = match deadline {
            Some(deadline) => wait.wait_until(deadline).map_err(ChannelError::from)?,
            None => {
                wait.wait().map_err(ChannelError::from)?;
                WaitOutcome::Notified
            }
        };
        let _interrupts = InterruptGuard::new();
        self.reference.0.lock().waker = None;
        if outcome == WaitOutcome::TimedOut {
            self.reference.fail(ChannelError::TimedOut);
        }
        self.receive_deliver(deliver)
    }
}

impl Deref for Call {
    type Target = CallReference;

    fn deref(&self) -> &Self::Target {
        &self.reference
    }
}

impl CallReference {
    pub fn try_receive(&self) -> Result<ReceivedMessage, TaskError> {
        self.receive_deliver(|_, _| Ok(()))
    }

    fn receive_deliver(
        &self,
        deliver: impl FnOnce(&ReceivedMessage, &[HandleEntry]) -> Result<(), TaskError>,
    ) -> Result<ReceivedMessage, TaskError> {
        let task = self.destination_task()?;
        task.receive_call_deliver(self, deliver)
    }

    pub(crate) fn destination_task(&self) -> Result<Task, ChannelError> {
        let _interrupts = InterruptGuard::new();
        self.0
            .lock()
            .destination
            .as_ref()
            .map(|destination| destination.task.clone())
            .ok_or(ChannelError::Closed)
    }

    pub(crate) fn complete(
        &self,
        commit: impl FnOnce(&mut Vec<HandleEntry>) -> super::Message,
    ) -> Result<(), ChannelError> {
        let mut state = self.0.lock();
        if !matches!(state.phase, CallPhase::Pending) {
            return Err(ChannelError::InvalidState);
        }
        if state.expired()? {
            state.finish(Err(ChannelError::TimedOut));
            return Err(ChannelError::InvalidState);
        }
        let message = commit(&mut state.entries);
        state.finish(Ok(message));
        Ok(())
    }

    pub(crate) fn receive_with(
        &self,
        table: &HandleTable,
        deliver: impl FnOnce(&ReceivedMessage, &[HandleEntry]) -> Result<(), TaskError>,
    ) -> Result<ReceivedMessage, TaskError> {
        let mut state = self.0.lock();
        let CallState {
            phase,
            destination,
            entries,
            ..
        } = &mut *state;
        match phase {
            CallPhase::Pending => Err(ChannelError::WouldBlock.into()),
            CallPhase::Finished => Err(ChannelError::InvalidState.into()),
            CallPhase::Ready(Err(error)) => {
                let error = *error;
                *phase = CallPhase::Finished;
                Err(error.into())
            }
            CallPhase::Ready(Ok(message)) => {
                let destination = destination.as_mut().unwrap();
                let count = entries.len();
                let received = ReceivedMessage::new(
                    *message,
                    destination.handles[..count].iter().copied().collect(),
                    false,
                );
                deliver(&received, entries)?;
                table.publish_received(received.handles(), entries)?;
                destination.handles.drain(..count);
                *phase = CallPhase::Finished;
                Ok(received)
            }
        }
    }

    pub fn cancel(&self) -> Result<(), ChannelError> {
        if !self.fail(ChannelError::Cancelled) {
            return Err(ChannelError::InvalidState);
        }
        Ok(())
    }

    pub(crate) fn fail(&self, error: ChannelError) -> bool {
        let _interrupts = InterruptGuard::new();
        {
            let mut state = self.0.lock();
            if !matches!(state.phase, CallPhase::Pending) {
                return false;
            }
            let error = if state.expired().unwrap_or(false) {
                ChannelError::TimedOut
            } else {
                error
            };
            state.finish(Err(error));
        }
        self.notify();
        true
    }

    pub(crate) fn identity(&self) -> usize {
        self.0.as_ptr().addr()
    }

    pub(crate) fn readiness(&self) -> Result<u64, ChannelError> {
        let state = self.0.lock();
        match state.phase {
            CallPhase::Pending => Ok(0),
            CallPhase::Ready(_) => Ok(super::READABLE),
            CallPhase::Finished => Err(ChannelError::Closed),
        }
    }

    pub(crate) fn register_observer(&self, observer: Observer) -> Result<(), MemoryError> {
        self.0.lock().observers.insert(observer)
    }

    pub(crate) fn unregister_observer(&self, observer: &Observer) {
        self.0.lock().observers.remove(observer);
    }

    pub(crate) fn notify(&self) {
        notify_observers(|index| self.0.lock().observers.get(index));
    }
}

impl CallState {
    fn expired(&self) -> Result<bool, ChannelError> {
        match self.deadline {
            Some(deadline) => Ok(hardware::timer()
                .now_ns()
                .map_err(crate::thread::ThreadError::from)?
                >= deadline),
            None => Ok(false),
        }
    }

    fn finish(&mut self, result: Result<super::Message, ChannelError>) {
        self.phase = CallPhase::Ready(result);
        if let Some(waker) = &self.waker {
            waker.wake();
        }
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        let _interrupts = InterruptGuard::new();
        let (destination, entries) = {
            let mut state = self.reference.0.lock();
            if let Some(waker) = state.waker.take() {
                waker.wake();
            }
            state.phase = CallPhase::Finished;
            (state.destination.take(), mem::take(&mut state.entries))
        };
        self.reference.notify();
        self.reference.0.lock().observers.clear();
        drop(entries);
        drop(destination);
    }
}

impl Drop for CallState {
    fn drop(&mut self) {
        CALLS.fetch_sub(1, Ordering::Relaxed);
    }
}
