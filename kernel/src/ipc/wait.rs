// SPDX-License-Identifier: MPL-2.0

use alloc::vec::Vec;
use spin::Mutex;

use super::{ChannelError, ChannelReference, EventError, EventReference};
use crate::{
    hardware::{self, cpu::InterruptGuard},
    memory::{self, HeapArc, MemoryError},
    task::{Handle, HandleError, ObjectReference, Task, TaskError, TaskState},
    thread::{self, ThreadError, ThreadState, Wait, WaitOutcome, Waker},
};

pub const READABLE: u64 = 1 << 0;
pub const WRITABLE: u64 = 1 << 1;
pub const PEER_CLOSED: u64 = 1 << 2;
pub const EXITED: u64 = 1 << 0;
pub const DEAD: u64 = 1 << 0;

const MAX_ITEMS: usize = 16;

#[derive(Clone, Copy)]
pub(crate) enum WaitMode {
    NonBlocking,
    Blocking,
    Deadline(u64),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WaitItem {
    pub handle: Handle,
    pub condition_mask: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WaitManyResult {
    pub index: usize,
    pub ready: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitManyError {
    InvalidArgument,
    WouldBlock,
    TimedOut,
    Closed,
    Handle(HandleError),
    Channel(ChannelError),
    Event(EventError),
    Thread(ThreadError),
    Memory(MemoryError),
}

enum Source {
    Channel(ChannelReference),
    Event(EventReference),
    Task(Task),
    Thread(thread::Thread),
}

struct ResolvedItem {
    source: Source,
    condition_mask: u64,
}

#[derive(Clone)]
pub(crate) struct Observer(HeapArc<Selection>);

pub(crate) struct Observers(Vec<Observer>);

struct Selection {
    items: Vec<ResolvedItem>,
    waker: Waker,
    result: Mutex<Option<Result<WaitManyResult, WaitManyError>>>,
}

struct Registrations {
    observer: Observer,
    count: usize,
}

pub(crate) fn wait_many(
    task: &Task,
    items: &[WaitItem],
    mode: WaitMode,
) -> Result<WaitManyResult, WaitManyError> {
    let interrupts = InterruptGuard::new();
    let items = resolve(task, items)?;
    if let Some(result) = ready(&items)? {
        consume(&items, result)?;
        return Ok(result);
    }
    let wait = prepare(mode)?;
    let observer = Observer(memory::manager().allocate_arc(Selection {
        items,
        waker: wait.waker(),
        result: Mutex::new(None),
    })?);
    let mut registrations = Registrations { observer, count: 0 };
    for item in &registrations.observer.0.items {
        item.source.register(registrations.observer.clone())?;
        registrations.count += 1;
    }
    drop(interrupts);
    match wait_for(mode, wait)? {
        WaitOutcome::Notified => registrations
            .observer
            .0
            .result
            .lock()
            .take()
            .expect("wait completed without a selection"),
        WaitOutcome::TimedOut => Err(WaitManyError::TimedOut),
    }
}

impl Observers {
    pub(crate) const fn new() -> Self {
        Self(Vec::new())
    }

    pub(crate) fn insert(&mut self, observer: Observer) -> Result<(), MemoryError> {
        self.0.try_reserve(1).map_err(|_| MemoryError::NoMemory)?;
        self.0.push(observer);
        Ok(())
    }

    pub(crate) fn remove(&mut self, observer: &Observer) {
        self.0
            .retain(|current| !HeapArc::ptr_eq(&current.0, &observer.0));
    }

    pub(crate) fn get(&self, index: usize) -> Option<Observer> {
        self.0.get(index).cloned()
    }
}

impl Observer {
    pub(crate) fn notify(&self) {
        let _interrupts = InterruptGuard::new();
        let mut result = self.0.result.lock();
        if result.is_some() {
            return;
        }
        let selected = match ready(&self.0.items) {
            Ok(Some(selected)) => Ok(selected),
            Ok(None) => return,
            Err(error) => Err(error),
        };
        // 以线程唤醒作为等待结果的提交点。
        if self.0.waker.wake() {
            *result = Some(selected.and_then(|selected| {
                consume(&self.0.items, selected)?;
                Ok(selected)
            }));
        }
    }
}

impl Source {
    fn register(&self, observer: Observer) -> Result<(), MemoryError> {
        match self {
            Self::Channel(channel) => channel.register_observer(observer),
            Self::Event(event) => event.register_observer(observer),
            Self::Task(task) => task.register_observer(observer),
            Self::Thread(thread) => thread.register_observer(observer),
        }
    }

    fn unregister(&self, observer: &Observer) {
        match self {
            Self::Channel(channel) => channel.unregister_observer(observer),
            Self::Event(event) => event.unregister_observer(observer),
            Self::Task(task) => task.unregister_observer(observer),
            Self::Thread(thread) => thread.unregister_observer(observer),
        }
    }

    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Channel(first), Self::Channel(second)) => first.same(second),
            (Self::Event(first), Self::Event(second)) => first.same(second),
            (Self::Task(first), Self::Task(second)) => first.same(second),
            (Self::Thread(first), Self::Thread(second)) => first.same(second),
            _ => false,
        }
    }
}

impl Drop for Registrations {
    fn drop(&mut self) {
        let _interrupts = InterruptGuard::new();
        for item in &self.observer.0.items[..self.count] {
            item.source.unregister(&self.observer);
        }
    }
}

fn resolve(task: &Task, items: &[WaitItem]) -> Result<Vec<ResolvedItem>, WaitManyError> {
    if items.is_empty() || items.len() > MAX_ITEMS {
        return Err(WaitManyError::InvalidArgument);
    }
    let mut resolved = Vec::<ResolvedItem>::new();
    resolved
        .try_reserve_exact(items.len())
        .map_err(|_| MemoryError::NoMemory)?;
    for item in items {
        if item.condition_mask == 0 {
            return Err(WaitManyError::InvalidArgument);
        }
        let source = match task.waitable(item.handle)? {
            ObjectReference::Channel(channel) => {
                if item.condition_mask & !(READABLE | WRITABLE | PEER_CLOSED) != 0 {
                    return Err(WaitManyError::InvalidArgument);
                }
                Source::Channel(channel)
            }
            ObjectReference::Event(event) => Source::Event(event),
            ObjectReference::Task(target) if item.condition_mask == EXITED => Source::Task(target),
            ObjectReference::Thread(target) if item.condition_mask == DEAD => {
                Source::Thread(target)
            }
            _ => return Err(WaitManyError::InvalidArgument),
        };
        if resolved
            .iter()
            .any(|previous| previous.source.same(&source))
        {
            return Err(WaitManyError::InvalidArgument);
        }
        resolved.push(ResolvedItem {
            source,
            condition_mask: item.condition_mask,
        });
    }
    Ok(resolved)
}

fn ready(items: &[ResolvedItem]) -> Result<Option<WaitManyResult>, WaitManyError> {
    for (index, item) in items.iter().enumerate() {
        let ready = match &item.source {
            Source::Channel(channel) => channel.readiness(item.condition_mask)?,
            Source::Event(event) => event.readiness(item.condition_mask)?,
            Source::Task(task) => {
                if matches!(task.state(), TaskState::Dead(_)) {
                    EXITED
                } else {
                    0
                }
            }
            Source::Thread(thread) => {
                if matches!(thread.state(), ThreadState::Dead(_)) {
                    DEAD
                } else {
                    0
                }
            }
        } & item.condition_mask;
        if ready != 0 {
            return Ok(Some(WaitManyResult { index, ready }));
        }
    }
    Ok(None)
}

fn consume(items: &[ResolvedItem], result: WaitManyResult) -> Result<(), WaitManyError> {
    if let Source::Event(event) = &items[result.index].source {
        event.consume(result.ready)?;
    }
    Ok(())
}

fn prepare(mode: WaitMode) -> Result<Wait, WaitManyError> {
    match mode {
        WaitMode::NonBlocking => Err(WaitManyError::WouldBlock),
        WaitMode::Blocking => thread::prepare_wait().map_err(Into::into),
        WaitMode::Deadline(deadline) => {
            if hardware::timer().now_ns()? >= deadline {
                Err(WaitManyError::TimedOut)
            } else {
                thread::prepare_wait().map_err(Into::into)
            }
        }
    }
}

fn wait_for(mode: WaitMode, wait: Wait) -> Result<WaitOutcome, WaitManyError> {
    match mode {
        WaitMode::NonBlocking => Err(WaitManyError::WouldBlock),
        WaitMode::Blocking => wait
            .wait()
            .map_err(Into::into)
            .map(|_| WaitOutcome::Notified),
        WaitMode::Deadline(deadline) => wait.wait_until(deadline).map_err(Into::into),
    }
}

impl From<HandleError> for WaitManyError {
    fn from(error: HandleError) -> Self {
        Self::Handle(error)
    }
}
impl From<ChannelError> for WaitManyError {
    fn from(error: ChannelError) -> Self {
        Self::Channel(error)
    }
}
impl From<EventError> for WaitManyError {
    fn from(error: EventError) -> Self {
        Self::Event(error)
    }
}
impl From<ThreadError> for WaitManyError {
    fn from(error: ThreadError) -> Self {
        Self::Thread(error)
    }
}
impl From<MemoryError> for WaitManyError {
    fn from(error: MemoryError) -> Self {
        Self::Memory(error)
    }
}
impl From<TaskError> for WaitManyError {
    fn from(error: TaskError) -> Self {
        match error {
            TaskError::InvalidState => Self::Closed,
            TaskError::Memory(error) => Self::Memory(error),
            TaskError::Handle(error) => Self::Handle(error),
            TaskError::Thread(error) => Self::Thread(error),
            TaskError::Channel(error) => Self::Channel(error),
            TaskError::Wait(error) => error,
        }
    }
}
impl From<hardware::timer::TimerError> for WaitManyError {
    fn from(error: hardware::timer::TimerError) -> Self {
        Self::Thread(ThreadError::Timer(error))
    }
}
