// SPDX-License-Identifier: MPL-2.0

//! 线程的生命周期、协作切换与等待唤醒。

mod deadline;
pub mod scheduler;
mod wait;

use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

use crate::{
    hardware::{
        self,
        cpu::InterruptState,
        execution::{Context, ContextStart, ExecutionError},
        mmu::PAGE_SIZE,
        timer::TimerError,
    },
    memory::{self, HeapArc, KernelStack, MemoryError},
};

pub use scheduler::{current, yield_now};
pub use wait::{Wait, WaitOutcome, Waker, prepare_wait, sleep_until};
pub(crate) use wait::{finish_task_exit, wait_for_task};

pub const MAX_THREADS: usize = 4096;
const STACK_SIZE: usize = 16 * 1024;
static THREADS: AtomicUsize = AtomicUsize::new(0);

pub type ThreadEntry = fn(usize) -> usize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThreadState {
    Created,
    Ready,
    Running,
    Blocked,
    Dying(usize),
    Dead(usize),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThreadError {
    NotInitialized,
    AlreadyInitialized,
    InvalidState,
    Capacity,
    GenerationExhausted,
    Memory(MemoryError),
    Execution(ExecutionError),
    Timer(TimerError),
}

impl From<MemoryError> for ThreadError {
    fn from(error: MemoryError) -> Self {
        Self::Memory(error)
    }
}

impl From<ExecutionError> for ThreadError {
    fn from(error: ExecutionError) -> Self {
        Self::Execution(error)
    }
}

impl From<TimerError> for ThreadError {
    fn from(error: TimerError) -> Self {
        Self::Timer(error)
    }
}

#[derive(Clone)]
pub struct Thread(HeapArc<ThreadInner>);

struct ThreadInner {
    entry: ThreadEntry,
    argument: usize,
    data: Mutex<ThreadData>,
    _capacity: ThreadCapacity,
}

struct ThreadData {
    state: ThreadState,
    resources: Option<ThreadResources>,
    ready_next: Option<Thread>,
    live_next: Option<Thread>,
    wait: Option<WaitState>,
    exit_waiters: Option<Thread>,
    generation: u64,
}

struct ThreadResources {
    context: Context,
    _stack: KernelStack,
}

struct WaitState {
    generation: u64,
    outcome: Option<WaitOutcome>,
    deadline_index: Option<usize>,
    exit: Option<ExitWait>,
}

struct ExitWait {
    target: wait::ExitTarget,
    previous: Option<Thread>,
    next: Option<Thread>,
}

struct ThreadCapacity;

struct InterruptGuard(InterruptState);

impl Thread {
    pub fn new(entry: ThreadEntry, argument: usize) -> Result<Self, ThreadError> {
        let _interrupts = InterruptGuard::new();
        scheduler::dispatcher()?;
        let capacity = ThreadCapacity::reserve()?;
        let stack = KernelStack::new((STACK_SIZE / PAGE_SIZE) as u64)?;
        let context = Context::new(ContextStart::Kernel {
            stack: stack.range(),
            entry: scheduler::enter,
            argument: 0,
        })?;
        let inner = memory::manager().allocate_arc(ThreadInner {
            entry,
            argument,
            data: Mutex::new(ThreadData {
                state: ThreadState::Created,
                resources: Some(ThreadResources {
                    context,
                    _stack: stack,
                }),
                ready_next: None,
                live_next: None,
                wait: None,
                exit_waiters: None,
                generation: 0,
            }),
            _capacity: capacity,
        })?;
        Ok(Self(inner))
    }

    pub fn start(&self) -> Result<(), ThreadError> {
        scheduler::start(self)
    }

    pub fn state(&self) -> ThreadState {
        let _interrupts = InterruptGuard::new();
        self.0.data.lock().state
    }

    pub(crate) fn same(&self, other: &Self) -> bool {
        HeapArc::ptr_eq(&self.0, &other.0)
    }
}

impl ThreadCapacity {
    fn reserve() -> Result<Self, ThreadError> {
        THREADS
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                (count < MAX_THREADS).then_some(count + 1)
            })
            .map_err(|_| ThreadError::Capacity)?;
        Ok(Self)
    }
}

impl Drop for ThreadCapacity {
    fn drop(&mut self) {
        THREADS.fetch_sub(1, Ordering::Relaxed);
    }
}

impl InterruptGuard {
    fn new() -> Self {
        Self(hardware::cpu().disable_interrupts())
    }
}

impl Drop for InterruptGuard {
    fn drop(&mut self) {
        hardware::cpu().restore_interrupts(self.0);
    }
}
