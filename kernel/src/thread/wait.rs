// SPDX-License-Identifier: MPL-2.0

use super::{
    Thread, ThreadError, ThreadState, WaitState,
    scheduler::{self, SuspendReason},
};

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
            notified: false,
        });
        Ok(Wait {
            thread: thread.clone(),
            generation,
        })
    })
}

impl Wait {
    pub fn waker(&self) -> Waker {
        Waker {
            thread: self.thread.clone(),
            generation: self.generation,
        }
    }

    pub fn wait(self) -> Result<(), ThreadError> {
        let notified = scheduler::with_scheduler(|scheduler| {
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
            Ok(wait.notified)
        })?;
        if !notified {
            scheduler::suspend(SuspendReason::Block)?;
        }
        Ok(())
    }
}

impl Drop for Wait {
    fn drop(&mut self) {
        scheduler::with_scheduler(|_| {
            let mut data = self.thread.0.data.lock();
            if data
                .wait
                .as_ref()
                .is_some_and(|wait| wait.generation == self.generation)
            {
                data.wait = None;
            }
        });
    }
}

impl Waker {
    pub fn wake(&self) -> bool {
        scheduler::with_scheduler(|scheduler| {
            let mut data = self.thread.0.data.lock();
            if matches!(data.state, ThreadState::Dying(_) | ThreadState::Dead(_)) {
                return false;
            }
            let Some(wait) = data.wait.as_mut() else {
                return false;
            };
            if wait.generation != self.generation || wait.notified {
                return false;
            }
            wait.notified = true;
            if data.state == ThreadState::Blocked {
                data.state = ThreadState::Ready;
                drop(data);
                scheduler.ready.push(self.thread.clone());
            }
            true
        })
    }
}
