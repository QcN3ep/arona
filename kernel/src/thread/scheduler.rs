// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Completely Fair Scheduler with virtual runtime and deadline-driven user preemption.

// TODO: Support CPU affinity and multiprocessor scheduling.

mod fair;
pub(super) use fair::Entity;
pub(super) use fair::Placement;

use spin::{Mutex, Once};

use super::{
    Thread, ThreadError, ThreadKind, ThreadResources, ThreadState, WaitOutcome,
    deadline::DeadlineQueue, ready::ReadyQueue,
};
use crate::hardware::{
    self,
    cpu::{InterruptGuard, InterruptState},
    execution::{Context, ContextStart},
    timer::TimerError,
};

static DISPATCHER: Once<Context> = Once::new();
static STATE: Mutex<Scheduler> = Mutex::new(Scheduler {
    ready: ReadyQueue::new(),
    load: LoadAverage {
        one: 0,
        five: 0,
        fifteen: 0,
        updated_ns: 0,
    },
    live: None,
    reap: None,
    current: None,
    request: None,
    preemption_requested: false,
    preemption_deadline: None,
    quantum_deadline: None,
    timer_target: None,
    deadlines: DeadlineQueue::new(),
    min_vruntime: 0,
    yielded: None,
});

pub(super) struct Scheduler {
    ready: ReadyQueue,
    live: Option<Thread>,
    reap: Option<Thread>,
    pub current: Option<Thread>,
    request: Option<SuspendReason>,
    preemption_requested: bool,
    preemption_deadline: Option<u64>,
    quantum_deadline: Option<u64>,
    timer_target: Option<u64>,
    pub deadlines: DeadlineQueue,
    load: LoadAverage,
    min_vruntime: i128,
    yielded: Option<u64>,
}

/// Fixed-point (16 fractional bits) runnable-thread averages updated by timer interrupts.
struct LoadAverage {
    one: u32,
    five: u32,
    fifteen: u32,
    updated_ns: u64,
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
/// Initialization and scheduling must run on the same processor. The exception entry and monotonic timer must be initialized before threads start. The current stack must stay alive as the scheduling stack.
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
    scheduler.account_current(hardware::timer().now_ns()?);
    let mut data = thread.0.data.lock();
    if data.state != ThreadState::Created {
        return Err(ThreadError::InvalidState);
    }
    data.state = ThreadState::Ready;
    data.live_next = scheduler.live.take();
    drop(data);
    scheduler.live = Some(thread.clone());
    scheduler.enqueue(thread.clone(), Placement::New);
    Ok(())
}

pub fn current() -> Option<Thread> {
    let _interrupts = InterruptGuard::new();
    STATE.lock().current.clone()
}

pub fn yield_now() -> Result<(), ThreadError> {
    suspend(SuspendReason::Yield)
}

pub(super) fn set_nice(thread: &Thread, nice: i8) -> Result<(), ThreadError> {
    if !(-20..=19).contains(&nice) {
        return Err(ThreadError::InvalidPriority);
    }
    change_fair(thread, |entity| entity.set_nice(nice))
}

pub(super) fn set_policy(
    thread: &Thread,
    policy: abi::thread::SchedulingPolicy,
) -> Result<(), ThreadError> {
    change_fair(thread, |entity| entity.set_policy(policy))
}

fn change_fair(thread: &Thread, change: impl FnOnce(&mut Entity)) -> Result<(), ThreadError> {
    with_scheduler(|scheduler| {
        let mut data = thread.0.data.lock();
        if matches!(data.state, ThreadState::Dying(_) | ThreadState::Dead(_)) {
            return Err(ThreadError::InvalidState);
        }
        let queued = data.state == ThreadState::Ready;
        drop(data);
        if queued {
            scheduler.ready.remove(thread);
        }
        data = thread.0.data.lock();
        change(&mut data.fair);
        drop(data);
        if queued {
            scheduler.ready.restore(thread.clone());
        }
        Ok(())
    })
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
    with_scheduler(|scheduler| scheduler.arm_timer())?;
    let requested = with_scheduler(|scheduler| {
        if !scheduler.preemption_requested
            || !scheduler.current.as_ref().is_some_and(|thread| {
                matches!(thread.0.kind, ThreadKind::User(_))
                    && thread.0.data.lock().state == ThreadState::Running
            })
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
        scheduler.account_current(hardware::timer().now_ns()?);
        scheduler
            .current
            .as_ref()
            .unwrap()
            .0
            .data
            .lock()
            .dispatched_ns = None;
        scheduler.request = Some(reason);
    }
    if let Err(error) = dispatcher.switch_from_current() {
        let mut scheduler = STATE.lock();
        scheduler.request = None;
        scheduler
            .current
            .as_ref()
            .unwrap()
            .0
            .data
            .lock()
            .dispatched_ns = Some(hardware::timer().now_ns()?);
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
    scheduler.account_current(now);
    scheduler.timer_target = None;
    while let Some(thread) = scheduler.deadlines.pop_due(now) {
        let generation = thread.0.data.lock().wait.as_ref().unwrap().generation;
        scheduler.finish_wait(&thread, generation, WaitOutcome::TimedOut);
    }
    let active = scheduler
        .ready
        .count
        .saturating_add(usize::from(scheduler.current.is_some()));
    scheduler.load.sample(now, active);
    if scheduler
        .preemption_deadline
        .is_some_and(|deadline| deadline <= now)
    {
        scheduler.preemption_deadline = None;
        scheduler.preemption_requested = true;
    }
    if scheduler
        .quantum_deadline
        .is_some_and(|deadline| deadline <= now)
    {
        scheduler.quantum_deadline = None;
        scheduler.preemption_requested = true;
    }
    scheduler.arm_timer()
}

pub(crate) fn refresh_timer() -> Result<(), ThreadError> {
    with_scheduler(|scheduler| scheduler.arm_timer())
}

/// Current 1, 5, and 15 minute runnable-thread averages in 16.16 fixed point.
pub fn load_average() -> [u32; 3] {
    let _interrupts = InterruptGuard::new();
    let scheduler = STATE.lock();
    [
        scheduler.load.one,
        scheduler.load.five,
        scheduler.load.fifteen,
    ]
}

pub fn run() -> ! {
    loop {
        run_ready().unwrap_or_else(|error| panic!("failed to schedule thread: {error:?}"));
        let _interrupts = InterruptGuard::new();
        if STATE.lock().ready.first().is_none() {
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
        let Some(mut thread) = scheduler.ready.pop() else {
            return Ok(false);
        };
        if scheduler.yielded.take() == Some(thread.id()) && scheduler.ready.count != 0 {
            let other = scheduler.ready.pop().unwrap();
            scheduler.ready.restore(thread);
            thread = other;
        }
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
            data.fair.slice_start = data.cpu_ns;
            (
                data.resources.take().expect("missing thread resources"),
                previous,
            )
        };
        scheduler.current = Some(thread.clone());
        if let Err(error) = scheduler.arm_timer() {
            scheduler.current = None;
            let mut data = thread.0.data.lock();
            data.resources = Some(resources);
            data.state = previous;
            drop(data);
            scheduler.ready.restore(thread);
            scheduler.quantum_deadline = None;
            return Err(error);
        }
        (thread, resources, previous)
    };
    let task = thread.task();
    let executed = core::mem::replace(&mut resources.executed, true);
    let result = task
        .as_ref()
        .map_or(Ok(()), |task| task.activate())
        .and_then(|()| {
            thread.0.data.lock().dispatched_ns = Some(hardware::timer().now_ns()?);
            dispatcher.switch_to(&resources.context).map_err(Into::into)
        });
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
        data.dispatched_ns = None;
        drop(data);
        scheduler.current = None;
        scheduler.ready.restore(thread);
        scheduler.quantum_deadline = None;
        scheduler.arm_timer()?;
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
    scheduler.quantum_deadline = None;
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
        if matches!(reason, SuspendReason::Yield) {
            scheduler.yielded = Some(thread.id());
        }
        scheduler.enqueue(thread, Placement::Requeue);
    }
    scheduler.update_min_vruntime();
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
            scheduler.enqueue(thread.clone(), Placement::Wake);
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
        scheduler.enqueue(thread.clone(), Placement::Wake);
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
    let mut scheduler = STATE.lock();
    if let Ok(now) = hardware::timer().now_ns() {
        scheduler.account_current(now);
    }
    action(&mut scheduler)
}

impl Scheduler {
    fn arm_timer(&mut self) -> Result<(), ThreadError> {
        let timer = hardware::timer();
        let now = timer.now_ns()?;
        self.account_current(now);
        self.refresh_slice(now);
        let target = match (self.deadlines.next_deadline(), self.preemption_deadline) {
            (Some(wait), Some(preemption)) => Some(wait.min(preemption)),
            (wait, preemption) => wait.or(preemption),
        };
        let target = match (target, self.quantum_deadline) {
            (Some(wait), Some(quantum)) => Some(wait.min(quantum)),
            (wait, quantum) => wait.or(quantum),
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

impl LoadAverage {
    fn sample(&mut self, now: u64, active: usize) {
        let elapsed = u128::from(now.saturating_sub(self.updated_ns));
        self.updated_ns = now;
        if elapsed == 0 {
            return;
        }
        let target = u128::from((active as u64).saturating_mul(1 << 16));
        for (value, period) in [
            (&mut self.one, 60_000_000_000u128),
            (&mut self.five, 300_000_000_000),
            (&mut self.fifteen, 900_000_000_000),
        ] {
            let current = u128::from(*value);
            // First-order low-pass: move by elapsed / (period + elapsed) toward the sample.
            let change = current.abs_diff(target) * elapsed / (period + elapsed);
            let next = if target >= current {
                current + change
            } else {
                current - change
            };
            *value = next.min(u128::from(u32::MAX)) as u32;
        }
    }
}
