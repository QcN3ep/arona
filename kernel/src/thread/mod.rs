// SPDX-License-Identifier: MPL-2.0

//! Thread lifecycle, switching, and waiting for wakeup.

mod deadline;
pub mod scheduler;
mod user;
mod wait;

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

use crate::{
    hardware::{
        self,
        cpu::InterruptGuard,
        execution::{Context, ContextStart, ExecutionError, UserEntry},
        mmu::PAGE_SIZE,
        timer::TimerError,
    },
    ipc::wait::{Observer, Observers, notify_observers},
    memory::{self, HeapArc, KernelStack, MemoryError},
};

pub use scheduler::{current, exit, yield_now};
pub(crate) use user::handle_trap;
pub use wait::{Wait, WaitOutcome, Waker, prepare_wait, sleep_until};
pub(crate) use wait::{finish_task_exit, wait_for_task};

pub const MAX_THREADS: usize = 4096;
const STACK_SIZE: usize = 32 * 1024;
static THREADS: AtomicUsize = AtomicUsize::new(0);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub fn allocated_threads() -> usize {
    THREADS.load(Ordering::Relaxed)
}

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
    Terminated,
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
    id: u64,
    kind: ThreadKind,
    data: Mutex<ThreadData>,
    _capacity: ThreadCapacity,
}

enum ThreadKind {
    Kernel { entry: ThreadEntry, argument: usize },
    User(crate::task::Task),
}

struct ThreadData {
    state: ThreadState,
    resources: Option<ThreadResources>,
    ready_next: Option<Thread>,
    live_next: Option<Thread>,
    task_next: Option<Thread>,
    reap_next: Option<Thread>,
    wait: Option<WaitState>,
    exit_waiters: Option<Thread>,
    observers: Observers,
    generation: u64,
    trapped: Option<u64>,
    interrupted: bool,
}

struct ThreadResources {
    context: Context,
    _stack: KernelStack,
    executed: bool,
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

impl Thread {
    pub fn id(&self) -> u64 {
        self.0.id
    }

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
        Self::create(
            ThreadKind::Kernel { entry, argument },
            capacity,
            stack,
            context,
        )
    }

    pub(crate) fn new_user(task: crate::task::Task, entry: UserEntry) -> Result<Self, ThreadError> {
        let _interrupts = InterruptGuard::new();
        scheduler::dispatcher()?;
        let capacity = ThreadCapacity::reserve()?;
        let stack = KernelStack::new((STACK_SIZE / PAGE_SIZE) as u64)?;
        let context = Context::new(ContextStart::User {
            stack: stack.range(),
            entry,
        })?;
        Self::create(ThreadKind::User(task), capacity, stack, context)
    }

    fn create(
        kind: ThreadKind,
        capacity: ThreadCapacity,
        stack: KernelStack,
        context: Context,
    ) -> Result<Self, ThreadError> {
        let id = NEXT_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| ThreadError::GenerationExhausted)?;
        let inner = memory::manager().allocate_arc(ThreadInner {
            id,
            kind,
            data: Mutex::new(ThreadData {
                state: ThreadState::Created,
                resources: Some(ThreadResources {
                    context,
                    _stack: stack,
                    executed: false,
                }),
                ready_next: None,
                live_next: None,
                task_next: None,
                reap_next: None,
                wait: None,
                exit_waiters: None,
                observers: Observers::new(),
                generation: 0,
                trapped: None,
                interrupted: false,
            }),
            _capacity: capacity,
        })?;
        Ok(Self(inner))
    }

    pub fn start(&self) -> Result<(), ThreadError> {
        match self.task() {
            Some(task) => task.start_thread(self),
            None => scheduler::start(self),
        }
    }

    pub fn terminate(&self, status: usize) -> Result<(), ThreadError> {
        let _interrupts = InterruptGuard::new();
        if self.task().is_none() || current().as_ref().is_some_and(|current| current.same(self)) {
            return Err(ThreadError::InvalidState);
        }
        scheduler::terminate(self, status);
        scheduler::reap_terminated();
        crate::task::reclaim_finished();
        Ok(())
    }

    pub fn task(&self) -> Option<crate::task::Task> {
        match &self.0.kind {
            ThreadKind::User(task) => Some(task.clone()),
            ThreadKind::Kernel { .. } => None,
        }
    }

    pub(crate) fn initial_entry(&self) -> Option<UserEntry> {
        self.0
            .data
            .lock()
            .resources
            .as_ref()
            .and_then(|resources| resources.context.user_state().ok())
            .and_then(|state| hardware::execution().user_entry(&state).ok())
    }

    pub(crate) fn task_next(&self) -> Option<Thread> {
        self.0.data.lock().task_next.clone()
    }

    pub(crate) fn set_task_next(&self, next: Option<Thread>) {
        self.0.data.lock().task_next = next;
    }

    pub fn state(&self) -> ThreadState {
        let _interrupts = InterruptGuard::new();
        self.0.data.lock().state
    }

    pub fn is_waiting_for_exit(&self) -> bool {
        self.0
            .data
            .lock()
            .wait
            .as_ref()
            .is_some_and(|wait| wait.exit.is_some())
    }

    pub(crate) fn same(&self, other: &Self) -> bool {
        HeapArc::ptr_eq(&self.0, &other.0)
    }

    pub(crate) fn identity(&self) -> usize {
        self.0.as_ptr().addr()
    }

    pub(crate) fn register_observer(&self, observer: Observer) -> Result<(), MemoryError> {
        self.0.data.lock().observers.insert(observer)
    }

    pub(crate) fn unregister_observer(&self, observer: &Observer) {
        self.0.data.lock().observers.remove(observer);
    }

    pub(crate) fn wake_waiters(&self) {
        notify_observers(|index| self.0.data.lock().observers.get(index));
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
