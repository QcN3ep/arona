// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! FIFO scheduler

// TODO: A better scheduling algorithm is needed
// TODO: Set a time slice when dispatching a Thread and reschedule when it expires.
// TODO: Support priorities, CPU affinity, and multiprocessor scheduling.

use spin::{Mutex, Once};

use super::{
    Thread, ThreadError, ThreadKind, ThreadResources, ThreadState, WaitOutcome,
    deadline::DeadlineQueue,
};
use crate::hardware::{
    self,
    cpu::{InterruptGuard, InterruptState},
    execution::{Context, ContextStart},
    timer::TimerError,
};

static DISPATCHER: Once<Context> = Once::new();
static STATE: Mutex<Scheduler> = Mutex::new(Scheduler {
    ready: ReadyQueue {
        first: None,
        last: None,
    },
    live: None,
    reap: None,
    current: None,
    request: None,
    preemption_requested: false,
    preemption_deadline: None,
    timer_target: None,
    deadlines: DeadlineQueue::new(),
});

pub(super) struct Scheduler {
    pub ready: ReadyQueue,
    live: Option<Thread>,
    reap: Option<Thread>,
    pub current: Option<Thread>,
    request: Option<SuspendReason>,
    preemption_requested: bool,
    preemption_deadline: Option<u64>,
    timer_target: Option<u64>,
    pub deadlines: DeadlineQueue,
}

pub(super) struct ReadyQueue {
    first: Option<Thread>,
    last: Option<Thread>,
}

#[derive(Clone, Copy)]
pub(super) enum SuspendReason {
    Yield,
    Preempt,
    Block,
    Exit(usize),
}

/// # Safety
///
/// Initialization and subsequent scheduling must run on the same processor; the exception entry must be installed, and the current stack must stay alive as the scheduling stack.
pub fn initialize() -> Result<(), ThreadError> {
    let _interrupts = InterruptGuard::new();
    let mut initialized = false;
    DISPATCHER.try_call_once(|| {
        let context = Context::new(ContextStart::Current)?;
        initialized = true;
        Ok::<_, ThreadError>(context)
    })?;
    if initialized {
        Ok(())
    } else {
        Err(ThreadError::AlreadyInitialized)
    }
}

pub(super) fn dispatcher() -> Result<&'static Context, ThreadError> {
    DISPATCHER.get().ok_or(ThreadError::NotInitialized)
}

pub(crate) fn start(thread: &Thread) -> Result<(), ThreadError> {
    let _interrupts = InterruptGuard::new();
    dispatcher()?;
    let mut scheduler = STATE.lock();
    let mut data = thread.0.data.lock();
    if data.state != ThreadState::Created {
        return Err(ThreadError::InvalidState);
    }
    data.state = ThreadState::Ready;
    data.live_next = scheduler.live.take();
    drop(data);
    scheduler.live = Some(thread.clone());
    scheduler.ready.push(thread.clone());
    Ok(())
}

pub fn current() -> Option<Thread> {
    let _interrupts = InterruptGuard::new();
    STATE.lock().current.clone()
}

pub fn yield_now() -> Result<(), ThreadError> {
    suspend(SuspendReason::Yield)
}

/// Requests rescheduling before returning to user mode; kernel execution is not switched out asynchronously.
pub fn request_preemption() -> Result<(), ThreadError> {
    dispatcher()?;
    with_scheduler(|scheduler| scheduler.preemption_requested = true);
    Ok(())
}

/// Sets a one-shot absolute deadline that is invalidated on switch-out; `None` cancels a request that has not yet expired.
pub fn set_preemption_deadline(deadline_ns: Option<u64>) -> Result<(), ThreadError> {
    dispatcher()?;
    with_scheduler(|scheduler| {
        let previous = scheduler.preemption_deadline;
        scheduler.preemption_deadline = deadline_ns;
        if let Err(error) = scheduler.arm_timer() {
            scheduler.preemption_deadline = previous;
            return Err(error);
        }
        Ok(())
    })
}

pub(super) fn preempt_if_requested() -> Result<(), ThreadError> {
    let _interrupts = InterruptGuard::new();
    let requested = with_scheduler(|scheduler| {
        if !scheduler.preemption_requested
            || !scheduler
                .current
                .as_ref()
                .is_some_and(|thread| matches!(thread.0.kind, ThreadKind::User(_)))
        {
            return false;
        }
        scheduler.preemption_requested = false;
        true
    });
    if requested && let Err(error) = suspend(SuspendReason::Preempt) {
        with_scheduler(|scheduler| scheduler.preemption_requested = true);
        return Err(error);
    }
    Ok(())
}

pub(super) fn suspend(reason: SuspendReason) -> Result<(), ThreadError> {
    let _interrupts = InterruptGuard::new();
    let dispatcher = dispatcher()?;
    {
        let mut scheduler = STATE.lock();
        let thread = scheduler
            .current
            .as_ref()
            .ok_or(ThreadError::InvalidState)?;
        let data = thread.0.data.lock();
        if matches!(data.state, ThreadState::Dying(_)) && !matches!(reason, SuspendReason::Exit(_))
        {
            return Err(ThreadError::Terminated);
        }
        if !matches!(data.state, ThreadState::Running | ThreadState::Dying(_))
            || scheduler.request.is_some()
        {
            return Err(ThreadError::InvalidState);
        }
        drop(data);
        scheduler.request = Some(reason);
    }
    if let Err(error) = dispatcher.switch_from_current() {
        let mut scheduler = STATE.lock();
        scheduler.request = None;
        return Err(error.into());
    }
    Ok(())
}

pub fn run_ready() -> Result<(), ThreadError> {
    let _interrupts = InterruptGuard::new();
    dispatcher()?;
    {
        let mut scheduler = STATE.lock();
        if scheduler.current.is_some() {
            return Err(ThreadError::InvalidState);
        }
        scheduler.arm_timer()?;
    }
    while dispatch()? {
        STATE.lock().arm_timer()?;
        // The Thread state has been committed and resources reclaimed; no lock is held here.
        hardware::cpu().restore_interrupts(InterruptState::Enabled);
        hardware::cpu().disable_interrupts();
    }
    Ok(())
}

pub(crate) fn timer_interrupt() -> Result<(), ThreadError> {
    let _interrupts = InterruptGuard::new();
    dispatcher()?;
    hardware::timer().acknowledge()?;
    let now = hardware::timer().now_ns()?;
    crate::ipc::timer_interrupt(now);
    let mut scheduler = STATE.lock();
    scheduler.timer_target = None;
    while let Some(thread) = scheduler.deadlines.pop_due(now) {
        let generation = thread.0.data.lock().wait.as_ref().unwrap().generation;
        scheduler.finish_wait(&thread, generation, WaitOutcome::TimedOut);
    }
    if scheduler
        .preemption_deadline
        .is_some_and(|deadline| deadline <= now)
    {
        scheduler.preemption_deadline = None;
        scheduler.preemption_requested = true;
    }
    scheduler.arm_timer()
}

pub(crate) fn refresh_timer() -> Result<(), ThreadError> {
    with_scheduler(|scheduler| scheduler.arm_timer())
}

pub fn run() -> ! {
    loop {
        run_ready().unwrap_or_else(|error| panic!("failed to schedule thread: {error:?}"));
        let _interrupts = InterruptGuard::new();
        if STATE.lock().ready.first.is_none() {
            hardware::cpu().idle_until_interrupt();
        }
    }
}

fn dispatch() -> Result<bool, ThreadError> {
    let _interrupts = InterruptGuard::new();
    let dispatcher = dispatcher()?;
    let (thread, mut resources, previous) = {
        let mut scheduler = STATE.lock();
        if scheduler.current.is_some() {
            return Err(ThreadError::InvalidState);
        }
        scheduler.preemption_requested = false;
        let Some(thread) = scheduler.ready.pop() else {
            return Ok(false);
        };
        let (resources, previous) = {
            let mut data = thread.0.data.lock();
            let previous = data.state;
            assert!(matches!(
                previous,
                ThreadState::Ready | ThreadState::Dying(_)
            ));
            if previous == ThreadState::Ready {
                data.state = ThreadState::Running;
            }
            (
                data.resources.take().expect("missing thread resources"),
                previous,
            )
        };
        scheduler.current = Some(thread.clone());
        (thread, resources, previous)
    };
    let task = thread.task();
    let executed = core::mem::replace(&mut resources.executed, true);
    let result = task
        .as_ref()
        .map_or(Ok(()), |task| task.activate())
        .and_then(|()| dispatcher.switch_to(&resources.context).map_err(Into::into));
    if task.is_some() {
        crate::memory::manager()
            .kernel_address_space()
            .activate()
            .expect("failed to restore kernel address space");
    }
    if let Err(error) = result {
        let mut scheduler = STATE.lock();
        let mut data = thread.0.data.lock();
        resources.executed = executed;
        data.resources = Some(resources);
        data.state = previous;
        drop(data);
        scheduler.current = None;
        scheduler.ready.push_front(thread);
        return Err(error);
    }
    complete(thread, resources);
    Ok(true)
}

fn complete(thread: Thread, resources: ThreadResources) {
    let mut scheduler = STATE.lock();
    let mut reason = scheduler
        .request
        .take()
        .expect("missing suspension request");
    scheduler.current = None;
    scheduler.preemption_requested = false;
    scheduler.preemption_deadline = None;
    let mut data = thread.0.data.lock();
    if let ThreadState::Dying(status) = data.state {
        reason = SuspendReason::Exit(status);
    }
    if let SuspendReason::Exit(status) = reason {
        data.state = ThreadState::Dying(status);
        data.wait = None;
        drop(data);
        scheduler.remove_live(&thread);
        drop(scheduler);
        drop(resources);
        crate::task::thread_exited(&thread, status);
        with_scheduler(|scheduler| scheduler.finish_exit(&thread, status));
        thread.wake_waiters();
        reap_terminated();
        crate::task::reclaim_finished();
        return;
    }
    data.resources = Some(resources);
    let ready = match reason {
        SuspendReason::Yield | SuspendReason::Preempt => true,
        SuspendReason::Block => data
            .wait
            .as_ref()
            .is_some_and(|wait| wait.outcome.is_some()),
        SuspendReason::Exit(_) => unreachable!(),
    };
    data.state = if ready {
        ThreadState::Ready
    } else {
        ThreadState::Blocked
    };
    drop(data);
    if ready {
        scheduler.ready.push(thread);
    }
}

pub(super) extern "C" fn enter(_: usize) -> ! {
    let (entry, argument) = {
        let scheduler = STATE.lock();
        let thread = scheduler.current.as_ref().expect("missing current thread");
        match thread.0.kind {
            ThreadKind::Kernel { entry, argument } => (entry, argument),
            ThreadKind::User(_) => unreachable!("user thread entered kernel entry"),
        }
    };
    let status = entry(argument);
    exit(status)
}

/// Terminates the current Thread without unwinding the current stack; local resources that need dropping must be released before calling.
pub fn exit(status: usize) -> ! {
    suspend(SuspendReason::Exit(status))
        .unwrap_or_else(|error| panic!("failed to exit thread: {error:?}"));
    panic!("exited thread resumed")
}

pub(crate) fn terminate(thread: &Thread, status: usize) {
    with_scheduler(|scheduler| {
        let mut data = thread.0.data.lock();
        let previous = data.state;
        if matches!(previous, ThreadState::Dying(_) | ThreadState::Dead(_)) {
            return;
        }
        let initial = data
            .resources
            .as_ref()
            .is_some_and(|resources| !resources.executed);
        data.state = ThreadState::Dying(status);
        drop(data);
        scheduler.cancel_wait(thread);
        if previous == ThreadState::Created || initial {
            if previous == ThreadState::Ready {
                scheduler.ready.remove(thread);
                scheduler.remove_live(thread);
            }
            thread.0.data.lock().reap_next = scheduler.reap.take();
            scheduler.reap = Some(thread.clone());
        } else if previous == ThreadState::Blocked {
            scheduler.ready.push(thread.clone());
        }
    });
}

pub(crate) fn suspend_interrupted(thread: &Thread, generation: u64) -> Result<(), ThreadError> {
    with_scheduler(|scheduler| {
        let mut data = thread.0.data.lock();
        if data.state != ThreadState::Ready || data.trapped.is_some() {
            return Err(ThreadError::InvalidState);
        }
        data.generation = generation;
        data.trapped = Some(generation);
        data.state = ThreadState::Blocked;
        drop(data);
        scheduler.ready.remove(thread);
        Ok(())
    })
}

pub(crate) fn resume_interrupted(thread: &Thread) {
    with_scheduler(|scheduler| {
        thread.0.data.lock().state = ThreadState::Ready;
        scheduler.ready.push(thread.clone());
    });
}

pub(crate) fn reap_terminated() {
    loop {
        let next = with_scheduler(|scheduler| {
            let thread = scheduler.reap.take()?;
            let mut data = thread.0.data.lock();
            scheduler.reap = data.reap_next.take();
            let ThreadState::Dying(status) = data.state else {
                unreachable!("invalid reap state");
            };
            let resources = data
                .resources
                .take()
                .expect("missing cancelled thread resources");
            drop(data);
            Some((thread, resources, status))
        });
        let Some((thread, resources, status)) = next else {
            break;
        };
        drop(resources);
        crate::task::thread_exited(&thread, status);
        with_scheduler(|scheduler| scheduler.finish_exit(&thread, status));
        thread.wake_waiters();
    }
}

pub(super) fn with_scheduler<T>(action: impl FnOnce(&mut Scheduler) -> T) -> T {
    let _interrupts = InterruptGuard::new();
    action(&mut STATE.lock())
}

impl Scheduler {
    fn arm_timer(&mut self) -> Result<(), ThreadError> {
        let timer = hardware::timer();
        let target = match (self.deadlines.next_deadline(), self.preemption_deadline) {
            (Some(wait), Some(preemption)) => Some(wait.min(preemption)),
            (wait, preemption) => wait.or(preemption),
        };
        let target = match (target, crate::ipc::next_timer_deadline()) {
            (Some(thread), Some(ipc)) => Some(thread.min(ipc)),
            (thread, ipc) => thread.or(ipc),
        };
        // Do not rearm for the same target, to avoid clearing an already-pending timer interrupt.
        if target == self.timer_target {
            return Ok(());
        }
        let Some(target) = target else {
            timer.cancel()?;
            self.timer_target = None;
            return Ok(());
        };
        let now = timer.now_ns()?;
        let mut deadline = target.max(now);
        loop {
            match timer.set_deadline(deadline) {
                // Time a long deadline in segments so that intermediate interrupts do not complete a wait that has not yet expired.
                Err(TimerError::DeadlineOutOfRange) if deadline > now => {
                    deadline = now + (deadline - now) / 2;
                }
                result => {
                    result?;
                    self.timer_target = Some(target);
                    return Ok(());
                }
            }
        }
    }

    fn remove_live(&mut self, target: &Thread) {
        let mut previous: Option<Thread> = None;
        let mut cursor = self.live.clone();
        while let Some(thread) = cursor {
            let next = thread.0.data.lock().live_next.clone();
            if thread.same(target) {
                if let Some(previous) = previous {
                    previous.0.data.lock().live_next = next;
                } else {
                    self.live = next;
                }
                thread.0.data.lock().live_next = None;
                return;
            }
            previous = Some(thread);
            cursor = next;
        }
        panic!("missing live thread");
    }
}

impl ReadyQueue {
    fn remove(&mut self, target: &Thread) {
        let mut previous: Option<Thread> = None;
        let mut cursor = self.first.clone();
        while let Some(thread) = cursor {
            let next = thread.0.data.lock().ready_next.clone();
            if thread.same(target) {
                if let Some(previous) = &previous {
                    previous.0.data.lock().ready_next = next.clone();
                } else {
                    self.first = next.clone();
                }
                if next.is_none() {
                    self.last = previous;
                }
                thread.0.data.lock().ready_next = None;
                return;
            }
            previous = Some(thread);
            cursor = next;
        }
        panic!("missing ready thread");
    }

    pub fn push(&mut self, thread: Thread) {
        if let Some(last) = &self.last {
            last.0.data.lock().ready_next = Some(thread.clone());
        } else {
            self.first = Some(thread.clone());
        }
        self.last = Some(thread);
    }

    fn push_front(&mut self, thread: Thread) {
        thread.0.data.lock().ready_next = self.first.take();
        if self.last.is_none() {
            self.last = Some(thread.clone());
        }
        self.first = Some(thread);
    }

    fn pop(&mut self) -> Option<Thread> {
        let thread = self.first.take()?;
        self.first = thread.0.data.lock().ready_next.take();
        if self.first.is_none() {
            self.last = None;
        }
        Some(thread)
    }
}
