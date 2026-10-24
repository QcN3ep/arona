// SPDX-License-Identifier: MPL-2.0

use spin::Mutex;

use super::{ChannelError, ReceivedMessage};
use crate::{
    hardware::{self, cpu::InterruptGuard},
    memory::{self, HeapArc},
    task::{Handle, ReservedHandles, Task, TaskError},
    thread::{self, Wait, WaitOutcome, Waker},
};

/// # Safety
///
/// 操作及析构期间须保留内存管理器所需的内核映射。
pub struct Reply {
    reference: ReplyReference,
}

/// # Safety
///
/// 操作及析构期间须保留内存管理器所需的内核映射。
#[derive(Clone)]
pub struct ReplyReference(HeapArc<Mutex<ReplyState>>);

struct ReplyState {
    destination: Option<ReservedHandles>,
    waker: Option<Waker>,
    deadline: Option<u64>,
    result: Option<Result<ReceivedMessage, ChannelError>>,
}

pub(crate) struct Call {
    reference: ReplyReference,
    reply: Option<Reply>,
    wait: Option<Wait>,
}

impl Call {
    pub(crate) fn new(task: &Task, deadline: Option<u64>) -> Result<Self, TaskError> {
        let destination = task.reserve_reply_handles()?;
        let reference = ReplyReference(memory::manager().allocate_arc(Mutex::new(ReplyState {
            destination: Some(destination),
            waker: None,
            deadline,
            result: None,
        }))?);
        Ok(Self {
            reply: Some(Reply {
                reference: reference.clone(),
            }),
            reference,
            wait: None,
        })
    }

    pub(crate) fn attach(&mut self) -> Result<Reply, ChannelError> {
        let wait = thread::prepare_wait()?;
        self.reference.0.lock().waker = Some(wait.waker());
        self.wait = Some(wait);
        Ok(self.reply.take().expect("call already submitted"))
    }

    pub(crate) fn wait(mut self) -> Result<ReceivedMessage, ChannelError> {
        let wait = self.wait.take().expect("call not submitted");
        let deadline = self.reference.0.lock().deadline;
        let outcome = match deadline {
            Some(deadline) => wait.wait_until(deadline)?,
            None => {
                wait.wait()?;
                WaitOutcome::Notified
            }
        };
        let _interrupts = InterruptGuard::new();
        if outcome == WaitOutcome::TimedOut {
            return Err(ChannelError::TimedOut);
        }
        let mut state = self.reference.0.lock();
        let result = state
            .result
            .replace(Err(ChannelError::InvalidState))
            .expect("reply wait completed without a result");
        if let Ok(response) = &result {
            state
                .destination
                .as_mut()
                .unwrap()
                .handles
                .drain(..response.handles().len());
        }
        result
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        let _interrupts = InterruptGuard::new();
        let (destination, waker) = {
            let mut state = self.reference.0.lock();
            state.result.get_or_insert(Err(ChannelError::InvalidState));
            (state.destination.take(), state.waker.take())
        };
        drop(self.wait.take());
        drop(waker);
        drop(destination);
    }
}

impl Reply {
    pub(crate) fn reference(&self) -> ReplyReference {
        self.reference.clone()
    }
}

impl ReplyReference {
    pub(crate) fn destination(&self) -> Result<Task, ChannelError> {
        let state = self.0.lock();
        if state.result.is_some() {
            return Err(ChannelError::InvalidState);
        }
        Ok(state
            .destination
            .as_ref()
            .ok_or(ChannelError::InvalidState)?
            .task
            .clone())
    }

    pub(crate) fn complete(
        &self,
        commit: impl FnOnce(&[Handle]) -> ReceivedMessage,
    ) -> Result<(), ChannelError> {
        let mut state = self.0.lock();
        if state.result.is_some() {
            return Err(ChannelError::InvalidState);
        }
        if let Some(deadline) = state.deadline
            && hardware::timer()
                .now_ns()
                .map_err(crate::thread::ThreadError::from)?
                >= deadline
        {
            state.result = Some(Err(ChannelError::TimedOut));
            state.waker.as_ref().unwrap().wake();
            return Err(ChannelError::InvalidState);
        }
        if !state.waker.as_ref().unwrap().wake() {
            return Err(ChannelError::InvalidState);
        }
        let response = commit(&state.destination.as_ref().unwrap().handles);
        state.result = Some(Ok(response));
        Ok(())
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        let _interrupts = InterruptGuard::new();
        let mut state = self.reference.0.lock();
        if state.result.is_none() {
            state.result = Some(Err(ChannelError::ServerGone));
            if let Some(waker) = &state.waker {
                waker.wake();
            }
        }
    }
}
