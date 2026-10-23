// SPDX-License-Identifier: MPL-2.0

use super::{
    Thread, ThreadError, ThreadState, WaitState,
    scheduler::{self, Scheduler, SuspendReason},
};
use crate::hardware;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitOutcome {
    Notified,
    TimedOut,
}

#[must_use]
pub struct Wait {
    thread: Thread,
    generation: u64,
}

#[derive(Clone)]
pub struct Waker {
    thread: Thread,
    generation: u64,
}

pub fn prepare_wait() -> Result<Wait, ThreadError> {
    scheduler::with_scheduler(|scheduler| {
        let thread = scheduler
            .current
            .as_ref()
            .ok_or(ThreadError::InvalidState)?;
        let mut data = thread.0.data.lock();
        if data.state != ThreadState::Running || data.wait.is_some() {
            return Err(ThreadError::InvalidState);
        }
        let generation = data
            .generation
            .checked_add(1)
            .ok_or(ThreadError::GenerationExhausted)?;
        data.generation = generation;
        data.wait = Some(WaitState {
            generation,
            outcome: None,
            deadline_index: None,
        });
        Ok(Wait {
            thread: thread.clone(),
            generation,
        })
    })
}

pub fn sleep_until(deadline_ns: u64) -> Result<(), ThreadError> {
    prepare_wait()?.wait_until(deadline_ns).map(|_| ())
}

impl Wait {
    pub fn waker(&self) -> Waker {
        Waker {
            thread: self.thread.clone(),
            generation: self.generation,
        }
    }

    pub fn wait(self) -> Result<(), ThreadError> {
        self.block(None).map(|_| ())
    }

    pub fn wait_until(self, deadline_ns: u64) -> Result<WaitOutcome, ThreadError> {
        self.block(Some(deadline_ns))
    }

    fn block(self, deadline: Option<u64>) -> Result<WaitOutcome, ThreadError> {
        let completed = scheduler::with_scheduler(|scheduler| {
            if !scheduler
                .current
                .as_ref()
                .is_some_and(|thread| thread.same(&self.thread))
            {
                return Err(ThreadError::InvalidState);
            }
            let data = self.thread.0.data.lock();
            if data.state != ThreadState::Running {
                return Err(ThreadError::InvalidState);
            }
            let wait = data.wait.as_ref().ok_or(ThreadError::InvalidState)?;
            if wait.generation != self.generation {
                return Err(ThreadError::InvalidState);
            }
            let outcome = wait.outcome;
            drop(data);
            if let Some(outcome) = outcome {
                return Ok(Some(outcome));
            }
            if let Some(deadline) = deadline {
                if deadline <= hardware::timer().now_ns()? {
                    scheduler.finish_wait(&self.thread, self.generation, WaitOutcome::TimedOut);
                    return Ok(Some(WaitOutcome::TimedOut));
                }
                scheduler.deadlines.insert(self.thread.clone(), deadline);
            }
            Ok(None)
        })?;
        if let Some(outcome) = completed {
            return Ok(outcome);
        }
        scheduler::suspend(SuspendReason::Block)?;
        scheduler::with_scheduler(|_| {
            let data = self.thread.0.data.lock();
            Ok(data.wait.as_ref().unwrap().outcome.unwrap())
        })
    }
}

impl Drop for Wait {
    fn drop(&mut self) {
        scheduler::with_scheduler(|scheduler| {
            let data = self.thread.0.data.lock();
            if !data
                .wait
                .as_ref()
                .is_some_and(|wait| wait.generation == self.generation)
            {
                return;
            }
            let index = data.wait.as_ref().unwrap().deadline_index;
            drop(data);
            if let Some(index) = index {
                scheduler.deadlines.remove(index);
            }
            self.thread.0.data.lock().wait = None;
        });
    }
}

impl Waker {
    pub fn wake(&self) -> bool {
        scheduler::with_scheduler(|scheduler| {
            scheduler.finish_wait(&self.thread, self.generation, WaitOutcome::Notified)
        })
    }
}

impl Scheduler {
    pub fn finish_wait(&mut self, thread: &Thread, generation: u64, outcome: WaitOutcome) -> bool {
        let index = {
            let mut data = thread.0.data.lock();
            let Some(wait) = data.wait.as_mut() else {
                return false;
            };
            if wait.generation != generation || wait.outcome.is_some() {
                return false;
            }
            wait.outcome = Some(outcome);
            wait.deadline_index
        };
        if let Some(index) = index {
            self.deadlines.remove(index);
        }
        let mut data = thread.0.data.lock();
        if data.state == ThreadState::Blocked {
            data.state = ThreadState::Ready;
            drop(data);
            self.ready.push(thread.clone());
        }
        true
    }
}
