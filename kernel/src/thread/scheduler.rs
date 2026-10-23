// SPDX-License-Identifier: MPL-2.0

//! FIFO 调度器

// TODO: 需要更好的调度算法

use spin::{Mutex, Once};

use super::{
    InterruptGuard, Thread, ThreadError, ThreadKind, ThreadResources, ThreadState, WaitOutcome,
    deadline::DeadlineQueue,
};
use crate::hardware::{
    self,
    cpu::InterruptState,
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
    deadlines: DeadlineQueue::new(),
});

pub(super) struct Scheduler {
    pub ready: ReadyQueue,
    live: Option<Thread>,
    reap: Option<Thread>,
    pub current: Option<Thread>,
    request: Option<SuspendReason>,
    pub deadlines: DeadlineQueue,
}

pub(super) struct ReadyQueue {
    first: Option<Thread>,
    last: Option<Thread>,
}

#[derive(Clone, Copy)]
pub(super) enum SuspendReason {
    Yield,
    Block,
    Exit(usize),
}

/// # Safety
///
/// 初始化及后续调度须在同一处理器执行，异常入口须已安装，当前栈须作为调度栈保持存活。
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
        let scheduler = STATE.lock();
        if scheduler.current.is_some() {
            return Err(ThreadError::InvalidState);
        }
        scheduler.arm_timer()?;
    }
    while dispatch()? {
        STATE.lock().arm_timer()?;
        // 线程状态已提交且资源已回收，此处不持有锁。
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
    let mut scheduler = STATE.lock();
    while let Some(thread) = scheduler.deadlines.pop_due(now) {
        let generation = thread.0.data.lock().wait.as_ref().unwrap().generation;
        scheduler.finish_wait(&thread, generation, WaitOutcome::TimedOut);
    }
    scheduler.arm_timer()
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
    let initial = resources.initial.take();
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
        resources.initial = initial;
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
        reap_terminated();
        crate::task::reclaim_finished();
        return;
    }
    data.resources = Some(resources);
    let ready = match reason {
        SuspendReason::Yield => true,
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

/// 结束当前线程，不展开当前栈；调用前须释放需要析构的局部资源。
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
            .is_some_and(|resources| resources.initial.is_some());
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
    }
}

pub(super) fn with_scheduler<T>(action: impl FnOnce(&mut Scheduler) -> T) -> T {
    let _interrupts = InterruptGuard::new();
    action(&mut STATE.lock())
}

impl Scheduler {
    fn arm_timer(&self) -> Result<(), ThreadError> {
        let timer = hardware::timer();
        let Some(deadline) = self.deadlines.next_deadline() else {
            return timer.cancel().map_err(Into::into);
        };
        let now = timer.now_ns()?;
        let mut deadline = deadline.max(now);
        loop {
            match timer.set_deadline(deadline) {
                // 长 deadline 分段计时，中间中断不完成尚未到期的等待。
                Err(TimerError::DeadlineOutOfRange) if deadline > now => {
                    deadline = now + (deadline - now) / 2;
                }
                result => return result.map_err(Into::into),
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
