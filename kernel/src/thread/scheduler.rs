// SPDX-License-Identifier: MPL-2.0

//! FIFO 调度器

use spin::{Mutex, Once};

use super::{InterruptGuard, Thread, ThreadError, ThreadResources, ThreadState};
use crate::hardware::{
    self,
    cpu::InterruptState,
    execution::{Context, ContextStart},
};

static DISPATCHER: Once<Context> = Once::new();
static STATE: Mutex<Scheduler> = Mutex::new(Scheduler {
    ready: ReadyQueue {
        first: None,
        last: None,
    },
    live: None,
    current: None,
    request: None,
});

pub(super) struct Scheduler {
    pub ready: ReadyQueue,
    live: Option<Thread>,
    pub current: Option<Thread>,
    request: Option<SuspendReason>,
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

pub(super) fn start(thread: &Thread) -> Result<(), ThreadError> {
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
        if data.state != ThreadState::Running || scheduler.request.is_some() {
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
    while dispatch()? {
        // 线程状态已提交且资源已回收，此处不持有锁。
        hardware::cpu().restore_interrupts(InterruptState::Enabled);
        hardware::cpu().disable_interrupts();
    }
    Ok(())
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
    let (thread, resources) = {
        let mut scheduler = STATE.lock();
        if scheduler.current.is_some() {
            return Err(ThreadError::InvalidState);
        }
        let Some(thread) = scheduler.ready.pop() else {
            return Ok(false);
        };
        let resources = {
            let mut data = thread.0.data.lock();
            assert_eq!(data.state, ThreadState::Ready);
            data.state = ThreadState::Running;
            data.resources.take().expect("missing thread resources")
        };
        scheduler.current = Some(thread.clone());
        (thread, resources)
    };
    if let Err(error) = dispatcher.switch_to(&resources.context) {
        let mut scheduler = STATE.lock();
        let mut data = thread.0.data.lock();
        data.resources = Some(resources);
        data.state = ThreadState::Ready;
        drop(data);
        scheduler.current = None;
        scheduler.ready.push_front(thread);
        return Err(error.into());
    }
    complete(thread, resources);
    Ok(true)
}

fn complete(thread: Thread, resources: ThreadResources) {
    let mut scheduler = STATE.lock();
    let reason = scheduler
        .request
        .take()
        .expect("missing suspension request");
    scheduler.current = None;
    let mut data = thread.0.data.lock();
    if let SuspendReason::Exit(status) = reason {
        data.state = ThreadState::Dying(status);
        data.wait = None;
        drop(data);
        scheduler.remove_live(&thread);
        drop(scheduler);
        drop(resources);
        thread.0.data.lock().state = ThreadState::Dead(status);
        return;
    }
    data.resources = Some(resources);
    let ready = match reason {
        SuspendReason::Yield => true,
        SuspendReason::Block => data.wait.as_ref().is_some_and(|wait| wait.notified),
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
        (thread.0.entry, thread.0.argument)
    };
    let status = entry(argument);
    suspend(SuspendReason::Exit(status))
        .unwrap_or_else(|error| panic!("failed to exit thread: {error:?}"));
    panic!("exited thread resumed")
}

pub(super) fn with_scheduler<T>(action: impl FnOnce(&mut Scheduler) -> T) -> T {
    let _interrupts = InterruptGuard::new();
    action(&mut STATE.lock())
}

impl Scheduler {
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
