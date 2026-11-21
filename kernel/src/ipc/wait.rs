// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

use super::{CallReference, ChannelError, ChannelReference, EventError, EventReference};
use crate::{
    hardware::{self, cpu::InterruptGuard},
    memory::{self, HeapArc, MemoryError},
    task::{Handle, HandleError, HandleTable, ObjectReference, Rights, Task, TaskError, TaskState},
    thread::{self, ThreadError, ThreadState, Wait, WaitOutcome, Waker},
};

pub const READABLE: u64 = 1 << 0;
pub const WRITABLE: u64 = 1 << 1;
pub const PEER_CLOSED: u64 = 1 << 2;
pub const EXITED: u64 = 1 << 0;
pub const DEAD: u64 = 1 << 0;

const WAIT_BATCH: usize = 128;

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
    Call(CallReference),
    Channel(ChannelReference),
    Event(EventReference),
    Task(Task),
    Thread(thread::Thread),
    TrapPort(crate::trap::port::TrapPortReference),
}

struct ResolvedItem {
    source: Source,
    condition_mask: u64,
}

#[derive(Clone)]
pub(crate) struct Observer {
    selection: HeapArc<Selection>,
    index: usize,
}

pub(crate) struct Observers(Vec<Observer>);

struct Selection {
    items: Vec<ResolvedItem>,
    waker: Waker,
    result: Mutex<Option<Result<WaitManyResult, WaitManyError>>>,
}

static SELECTIONS: AtomicUsize = AtomicUsize::new(0);

pub fn active_selections() -> usize {
    SELECTIONS.load(Ordering::Relaxed)
}

struct Registrations {
    observer: Observer,
    count: usize,
    wait: Option<Wait>,
}

pub(crate) fn wait_many(
    task: &Task,
    count: usize,
    mut read: impl FnMut(usize) -> Result<WaitItem, WaitManyError>,
    mode: WaitMode,
) -> Result<WaitManyResult, WaitManyError> {
    let resolved = resolve(task, count, &mut read)?;
    drop(read);
    wait_resolved(resolved, mode)
}

fn wait_resolved(
    items: Vec<ResolvedItem>,
    mode: WaitMode,
) -> Result<WaitManyResult, WaitManyError> {
    for (index, item) in items.iter().enumerate() {
        checkpoint(index)?;
        let _interrupts = InterruptGuard::new();
        match item.ready(index) {
            Ok(Some(result)) => {
                consume(&items, result)?;
                return Ok(result);
            }
            Ok(None) => {}
            Err(error) => return Err(error),
        }
    }
    let wait = prepare(mode)?;
    SELECTIONS.fetch_add(1, Ordering::Relaxed);
    let selection = memory::manager().allocate_arc(Selection {
        items,
        waker: wait.waker(),
        result: Mutex::new(None),
    })?;
    let observer = Observer {
        selection,
        index: 0,
    };
    let mut registrations = Registrations {
        observer,
        count: 0,
        wait: Some(wait),
    };
    let mut observer = registrations.observer.clone();
    let selection = &*registrations.observer.selection;
    for index in 0..selection.items.len() {
        checkpoint(index)?;
        let _interrupts = InterruptGuard::new();
        if selection.result.lock().is_some() {
            break;
        }
        observer.index = index;
        selection.items[index].source.register(observer.clone())?;
        registrations.count += 1;
        observer.notify();
    }
    let result = match wait_for(mode, registrations.wait.take().unwrap())? {
        WaitOutcome::Notified => {
            (*selection.result.lock()).expect("wait completed without a selection")
        }
        WaitOutcome::TimedOut => Err(WaitManyError::TimedOut),
    };
    drop(registrations);
    result
}

pub(crate) fn notify_observers(mut get: impl FnMut(usize) -> Option<Observer>) {
    let _interrupts = InterruptGuard::new();
    let mut index = 0;
    // Notification re-queries the object, so the object lock must be released before the getter closure returns.
    while let Some(observer) = get(index) {
        observer.notify();
        index += 1;
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
            .retain(|current| !HeapArc::ptr_eq(&current.selection, &observer.selection));
    }

    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    pub(crate) fn get(&self, index: usize) -> Option<Observer> {
        self.0.get(index).cloned()
    }
}

impl Observer {
    pub(crate) fn notify(&self) {
        let _interrupts = InterruptGuard::new();
        let mut result = self.selection.result.lock();
        if result.is_some() {
            return;
        }
        let selected = match self.selection.items[self.index].ready(self.index) {
            Ok(Some(selected)) => Ok(selected),
            Ok(None) => return,
            Err(error) => Err(error),
        };
        // Thread wakeup is the commit point for the wait result.
        if self.selection.waker.wake() {
            *result = Some(selected.and_then(|selected| {
                consume(&self.selection.items, selected)?;
                Ok(selected)
            }));
        }
    }
}

impl Drop for Selection {
    fn drop(&mut self) {
        SELECTIONS.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Source {
    fn register(&self, observer: Observer) -> Result<(), MemoryError> {
        match self {
            Self::Call(call) => call.register_observer(observer),
            Self::Channel(channel) => channel.register_observer(observer),
            Self::Event(event) => event.register_observer(observer),
            Self::Task(task) => task.register_observer(observer),
            Self::Thread(thread) => thread.register_observer(observer),
            Self::TrapPort(port) => port.register_observer(observer),
        }
    }

    fn unregister(&self, observer: &Observer) {
        match self {
            Self::Call(call) => call.unregister_observer(observer),
            Self::Channel(channel) => channel.unregister_observer(observer),
            Self::Event(event) => event.unregister_observer(observer),
            Self::Task(task) => task.unregister_observer(observer),
            Self::Thread(thread) => thread.unregister_observer(observer),
            Self::TrapPort(port) => port.unregister_observer(observer),
        }
    }

    fn identity(&self) -> usize {
        // The reference keeps the storage alive; Channel additionally uses the lowest bit to distinguish endpoints.
        match self {
            Self::Call(call) => call.identity(),
            Self::Channel(channel) => channel.identity(),
            Self::Event(event) => event.identity(),
            Self::Task(task) => task.identity(),
            Self::Thread(thread) => thread.identity(),
            Self::TrapPort(port) => port.identity(),
        }
    }
}

impl Drop for Registrations {
    fn drop(&mut self) {
        drop(self.wait.take());
        for (index, item) in self.observer.selection.items[..self.count]
            .iter()
            .enumerate()
        {
            let _ = checkpoint(index);
            let _interrupts = InterruptGuard::new();
            item.source.unregister(&self.observer);
        }
    }
}

fn resolve(
    task: &Task,
    count: usize,
    read: &mut dyn FnMut(usize) -> Result<WaitItem, WaitManyError>,
) -> Result<Vec<ResolvedItem>, WaitManyError> {
    if count == 0 {
        return Err(WaitManyError::InvalidArgument);
    }
    let mut resolved = Vec::<ResolvedItem>::new();
    resolved
        .try_reserve_exact(count)
        .map_err(|_| MemoryError::NoMemory)?;
    if count == 1 {
        let item = read(0)?;
        if item.condition_mask == 0 {
            return Err(WaitManyError::InvalidArgument);
        }
        let object = if item.handle == Handle::CURRENT_THREAD {
            ObjectReference::Thread(thread::current().ok_or(ThreadError::InvalidState)?)
        } else {
            task.with_handles(|handles| handles.get_any(item.handle, Rights::WAIT))??
        };
        resolved.push(ResolvedItem {
            source: source(object, item.condition_mask)?,
            condition_mask: item.condition_mask,
        });
        return Ok(resolved);
    }
    let mut identities = Vec::new();
    identities
        .try_reserve_exact(count)
        .map_err(|_| MemoryError::NoMemory)?;
    resolve_batches(task, count, read, &mut resolved, &mut identities)?;
    let identities = identities.as_mut_slice();
    for root in (0..identities.len() / 2).rev() {
        checkpoint(root)?;
        sift_down(identities, root);
    }
    for end in (1..identities.len()).rev() {
        checkpoint(end)?;
        let identity = identities[0];
        identities[0] = identities[end];
        let heap = &mut identities[..end];
        sift_down(heap, 0);
        if heap[0] == identity {
            return Err(WaitManyError::InvalidArgument);
        }
    }
    Ok(resolved)
}

fn resolve_batches(
    task: &Task,
    count: usize,
    read: &mut dyn FnMut(usize) -> Result<WaitItem, WaitManyError>,
    resolved: &mut Vec<ResolvedItem>,
    identities: &mut Vec<usize>,
) -> Result<(), WaitManyError> {
    for base in (0..count).step_by(WAIT_BATCH) {
        checkpoint(base)?;
        let mut batch = arrayvec::ArrayVec::<WaitItem, WAIT_BATCH>::new();
        let mut current = None;
        let mut needs_handles = false;
        for index in base..(base + WAIT_BATCH).min(count) {
            let item = read(index)?;
            if item.condition_mask == 0 {
                return Err(WaitManyError::InvalidArgument);
            }
            if item.handle == Handle::CURRENT_THREAD {
                current = Some(thread::current().ok_or(ThreadError::InvalidState)?);
            } else {
                needs_handles = true;
            }
            batch.push(item);
        }
        if needs_handles {
            task.with_handles(|handles| {
                resolve_batch(
                    &batch,
                    current.as_ref(),
                    Some(handles),
                    resolved,
                    identities,
                )
            })??;
        } else {
            resolve_batch(&batch, current.as_ref(), None, resolved, identities)?;
        }
    }
    Ok(())
}

fn resolve_batch(
    batch: &[WaitItem],
    current: Option<&thread::Thread>,
    handles: Option<&HandleTable>,
    resolved: &mut Vec<ResolvedItem>,
    identities: &mut Vec<usize>,
) -> Result<(), WaitManyError> {
    for item in batch {
        let object = if item.handle == Handle::CURRENT_THREAD {
            ObjectReference::Thread(current.unwrap().clone())
        } else {
            handles.unwrap().get_any(item.handle, Rights::WAIT)?
        };
        let source = source(object, item.condition_mask)?;
        identities.push(source.identity());
        resolved.push(ResolvedItem {
            source,
            condition_mask: item.condition_mask,
        });
    }
    Ok(())
}

fn source(object: ObjectReference, condition_mask: u64) -> Result<Source, WaitManyError> {
    Ok(match object {
        ObjectReference::Call(call) if condition_mask == READABLE => Source::Call(call),
        ObjectReference::Channel(channel) => {
            if condition_mask & !(READABLE | WRITABLE | PEER_CLOSED) != 0 {
                return Err(WaitManyError::InvalidArgument);
            }
            Source::Channel(channel)
        }
        ObjectReference::Event(event) => Source::Event(event),
        ObjectReference::TrapPort(port) if condition_mask == READABLE => Source::TrapPort(port),
        ObjectReference::Task(target) if condition_mask == EXITED => Source::Task(target),
        ObjectReference::Thread(target) if condition_mask == DEAD => Source::Thread(target),
        _ => return Err(WaitManyError::InvalidArgument),
    })
}

// REFERENCE: https://en.wikipedia.org/wiki/Heapsort
fn sift_down(heap: &mut [usize], mut root: usize) {
    let identity = heap[root];
    while root < heap.len() / 2 {
        let mut child = root * 2 + 1;
        if child + 1 < heap.len() && heap[child] < heap[child + 1] {
            child += 1;
        }
        if identity >= heap[child] {
            break;
        }
        heap[root] = heap[child];
        root = child;
    }
    heap[root] = identity;
}

fn checkpoint(index: usize) -> Result<(), ThreadError> {
    // Interrupts cannot be enabled directly inside a trap; pending interrupts are handled through the scheduling stack.
    if index != 0 && index.is_multiple_of(WAIT_BATCH) && thread::current().is_some() {
        thread::yield_now()?;
    }
    Ok(())
}

impl ResolvedItem {
    fn ready(&self, index: usize) -> Result<Option<WaitManyResult>, WaitManyError> {
        let ready = match &self.source {
            Source::Call(call) => call.readiness()?,
            Source::Channel(channel) => channel.readiness(self.condition_mask)?,
            Source::Event(event) => event.readiness(self.condition_mask)?,
            Source::TrapPort(port) => port.readiness().map_err(|_| WaitManyError::Closed)?,
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
        } & self.condition_mask;
        if ready == 0 {
            Ok(None)
        } else {
            Ok(Some(WaitManyResult { index, ready }))
        }
    }
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

impl WaitMode {
    pub(crate) fn deadline(self) -> Option<u64> {
        match self {
            Self::Deadline(deadline) => Some(deadline),
            Self::NonBlocking | Self::Blocking => None,
        }
    }
}

fn wait_for(mode: WaitMode, wait: Wait) -> Result<WaitOutcome, WaitManyError> {
    if matches!(mode, WaitMode::NonBlocking) {
        return Err(WaitManyError::WouldBlock);
    }
    wait.block(mode.deadline()).map_err(Into::into)
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
