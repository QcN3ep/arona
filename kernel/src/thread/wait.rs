// SPDX-License-Identifier: MPL-2.0

use super::{
    ExitWait, Thread, ThreadError, ThreadState, WaitState,
    scheduler::{self, Scheduler, SuspendReason},
};
use crate::{
    hardware,
    task::{Task, TaskState},
};

#[derive(Clone)]
pub(super) enum ExitTarget {
    Thread(Thread),
    Task(Task),
}

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
            exit: None,
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

impl Thread {
    pub fn wait(&self) -> Result<usize, ThreadError> {
        Ok(ExitTarget::Thread(self.clone()).wait(None)?.unwrap())
    }

    /// 等待退出，超时返回 None。
    pub fn wait_until(&self, deadline_ns: u64) -> Result<Option<usize>, ThreadError> {
        ExitTarget::Thread(self.clone()).wait(Some(deadline_ns))
    }
}

pub(crate) fn wait_for_task(
    task: Task,
    deadline: Option<u64>,
) -> Result<Option<usize>, ThreadError> {
    ExitTarget::Task(task).wait(deadline)
}

pub(crate) fn finish_task_exit(task: &Task, status: usize) {
    scheduler::with_scheduler(|scheduler| {
        scheduler.finish_target_exit(ExitTarget::Task(task.clone()), status)
    });
}

impl ExitTarget {
    fn wait(self, deadline: Option<u64>) -> Result<Option<usize>, ThreadError> {
        let wait = self.prepare_exit_wait(deadline)?;
        let outcome = match deadline {
            Some(deadline) => wait.wait_until(deadline)?,
            None => {
                wait.wait()?;
                WaitOutcome::Notified
            }
        };
        match outcome {
            WaitOutcome::Notified => Ok(Some(
                self.exit_status()
                    .expect("exit wait completed before target died"),
            )),
            WaitOutcome::TimedOut => Ok(None),
        }
    }

    fn prepare_exit_wait(&self, deadline: Option<u64>) -> Result<Wait, ThreadError> {
        let wait = prepare_wait()?;
        scheduler::with_scheduler(|scheduler| {
            if matches!(self, Self::Thread(thread) if thread.same(&wait.thread)) {
                return Err(ThreadError::InvalidState);
            }
            let outcome = if self.exit_status().is_some() {
                Some(WaitOutcome::Notified)
            } else if let Some(deadline) = deadline {
                (deadline <= hardware::timer().now_ns()?).then_some(WaitOutcome::TimedOut)
            } else {
                None
            };
            if let Some(outcome) = outcome {
                scheduler.finish_wait(&wait.thread, wait.generation, outcome);
                return Ok(());
            }
            self.with_waiters(|waiters| {
                let next = waiters.take();
                if let Some(next) = &next {
                    let mut next = next.0.data.lock();
                    let exit = next.wait.as_mut().unwrap().exit.as_mut().unwrap();
                    exit.previous = Some(wait.thread.clone());
                }
                wait.thread.0.data.lock().wait.as_mut().unwrap().exit = Some(ExitWait {
                    target: self.clone(),
                    previous: None,
                    next,
                });
                *waiters = Some(wait.thread.clone());
            });
            Ok(())
        })?;
        Ok(wait)
    }

    fn exit_status(&self) -> Option<usize> {
        match self {
            Self::Thread(thread) => match thread.state() {
                ThreadState::Dead(status) => Some(status),
                _ => None,
            },
            Self::Task(task) => match task.state() {
                TaskState::Dead(status) => Some(status),
                _ => None,
            },
        }
    }

    fn with_waiters<T>(&self, action: impl FnOnce(&mut Option<Thread>) -> T) -> T {
        match self {
            Self::Thread(thread) => action(&mut thread.0.data.lock().exit_waiters),
            Self::Task(task) => task.with_exit_waiters(action),
        }
    }
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
            scheduler.remove_exit_wait(&self.thread);
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
        self.remove_exit_wait(thread);
        let mut data = thread.0.data.lock();
        if data.state == ThreadState::Blocked {
            data.state = ThreadState::Ready;
            drop(data);
            self.ready.push(thread.clone());
        }
        true
    }

    pub fn finish_exit(&mut self, thread: &Thread, status: usize) {
        self.finish_target_exit(ExitTarget::Thread(thread.clone()), status);
    }

    fn finish_target_exit(&mut self, target: ExitTarget, status: usize) {
        match &target {
            ExitTarget::Thread(thread) => thread.0.data.lock().state = ThreadState::Dead(status),
            ExitTarget::Task(task) => task.finish_exit(status),
        }
        loop {
            let waiter = target.with_waiters(|waiters| waiters.clone());
            let Some(waiter) = waiter else {
                break;
            };
            let generation = waiter.0.data.lock().wait.as_ref().unwrap().generation;
            assert!(self.finish_wait(&waiter, generation, WaitOutcome::Notified));
        }
    }

    fn remove_exit_wait(&mut self, thread: &Thread) {
        let exit = thread.0.data.lock().wait.as_mut().unwrap().exit.take();
        let Some(exit) = exit else {
            return;
        };
        if let Some(previous) = &exit.previous {
            let mut previous = previous.0.data.lock();
            previous.wait.as_mut().unwrap().exit.as_mut().unwrap().next = exit.next.clone();
        } else {
            exit.target
                .with_waiters(|waiters| *waiters = exit.next.clone());
        }
        if let Some(next) = exit.next {
            let mut next = next.0.data.lock();
            next.wait.as_mut().unwrap().exit.as_mut().unwrap().previous = exit.previous;
        }
    }
}
