// SPDX-License-Identifier: MPL-2.0

//! Task 的创建归属、运行资源回收与退出等待。

use arrayvec::ArrayVec;
use core::{
    ops::Range,
    sync::atomic::{AtomicU64, Ordering},
};
use spin::Mutex;

use super::{
    Handle, HandleError, HandleTable, Object, ObjectReference, ObjectType, Rights, TransferRequest,
};
use crate::{
    hardware::{
        self,
        cpu::InterruptGuard,
        execution::{ExecutionError, UserEntry},
        mmu::PagePermissions,
    },
    ipc::{
        Call, ChannelError, ChannelReference, MESSAGE_HANDLES, Message, ReceivedMessage, WaitItem,
        WaitManyError, WaitManyResult, WaitMode,
        wait::{Observer, Observers, notify_observers},
    },
    memory::{
        self, AddressSpace, FrameAllocation, HeapArc, HeapBox, MappingInfo, MappingOptions,
        MemoryError, VirtRange,
    },
    thread::{self, Thread, ThreadError, ThreadState, scheduler},
};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

static TASKS: Mutex<TaskRegistry> = Mutex::new(TaskRegistry { first: None });

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskState {
    Created,
    Active,
    Dying(usize),
    Dead(usize),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskError {
    InvalidState,
    Memory(MemoryError),
    Handle(HandleError),
    Thread(ThreadError),
    Channel(ChannelError),
    Wait(WaitManyError),
}

impl From<ChannelError> for TaskError {
    fn from(error: ChannelError) -> Self {
        Self::Channel(error)
    }
}

impl From<WaitManyError> for TaskError {
    fn from(error: WaitManyError) -> Self {
        Self::Wait(error)
    }
}

impl From<MemoryError> for TaskError {
    fn from(error: MemoryError) -> Self {
        Self::Memory(error)
    }
}

impl From<HandleError> for TaskError {
    fn from(error: HandleError) -> Self {
        Self::Handle(error)
    }
}

impl From<ThreadError> for TaskError {
    fn from(error: ThreadError) -> Self {
        Self::Thread(error)
    }
}

/// # Safety
///
/// 须在调度器所属处理器操作并保留内核映射，控制引用析构时须关闭本地中断。
#[derive(Clone)]
pub struct Task(HeapArc<TaskRecord>);

struct TaskRecord {
    id: u64,
    data: Mutex<TaskData>,
}

struct TaskData {
    state: TaskState,
    exit_waiters: Option<Thread>,
    observers: Observers,
}

struct TaskRegistry {
    first: Option<HeapBox<TaskResources>>,
}

struct TaskResources {
    task: Task,
    creator: Option<Creator>,
    space: AddressSpace,
    handles: HandleTable,
    threads: Option<Thread>,
    next: Option<HeapBox<TaskResources>>,
}

enum Creator {
    Thread(Thread),
    Task(Task),
}

pub(crate) struct ReservedHandles {
    pub(crate) task: Task,
    pub(crate) handles: ArrayVec<Handle, MESSAGE_HANDLES>,
}

impl Task {
    pub fn new() -> Result<Self, TaskError> {
        let _interrupts = InterruptGuard::new();
        let creator = thread::current().ok_or(TaskError::InvalidState)?;
        let owner = creator
            .task()
            .map_or(Creator::Thread(creator), Creator::Task);
        TASKS.lock().create(owner)
    }

    pub fn create_child(&self) -> Result<Self, TaskError> {
        let _interrupts = InterruptGuard::new();
        TASKS.lock().create(Creator::Task(self.clone()))
    }

    pub fn state(&self) -> TaskState {
        let _interrupts = InterruptGuard::new();
        self.0.data.lock().state
    }

    pub fn insert(&self, incoming: Object, rights: Rights) -> Result<Handle, TaskError> {
        let _interrupts = InterruptGuard::new();
        let object = incoming;
        let registry = TASKS.lock();
        Ok(registry.find(self)?.handles.insert(object, rights)?)
    }

    pub(crate) fn handle_info(&self, handle: Handle) -> Result<(ObjectType, Rights), TaskError> {
        let _interrupts = InterruptGuard::new();
        Ok(TASKS.lock().find(self)?.handles.info(handle)?)
    }

    pub fn get(
        &self,
        handle: Handle,
        kind: ObjectType,
        rights: Rights,
    ) -> Result<ObjectReference, TaskError> {
        let _interrupts = InterruptGuard::new();
        let registry = TASKS.lock();
        Ok(registry.find(self)?.handles.get(handle, kind, rights)?)
    }

    pub(crate) fn waitable(&self, handle: Handle) -> Result<ObjectReference, TaskError> {
        let _interrupts = InterruptGuard::new();
        let registry = TASKS.lock();
        Ok(registry.find(self)?.handles.get_any(handle, Rights::WAIT)?)
    }

    pub fn close(&self, handle: Handle) -> Result<(), TaskError> {
        let _interrupts = InterruptGuard::new();
        let registry = TASKS.lock();
        Ok(registry.find(self)?.handles.close(handle)?)
    }

    pub fn duplicate(&self, handle: Handle, rights: Rights) -> Result<Handle, TaskError> {
        let _interrupts = InterruptGuard::new();
        let registry = TASKS.lock();
        Ok(registry.find(self)?.handles.duplicate(handle, rights)?)
    }

    pub fn install_handle(
        &self,
        source: &Self,
        request: TransferRequest,
    ) -> Result<Handle, TaskError> {
        let _interrupts = InterruptGuard::new();
        let registry = TASKS.lock();
        let resources = registry.find(self)?;
        if self.state() != TaskState::Created {
            return Err(TaskError::InvalidState);
        }
        let source = &registry.find(source)?.handles;
        let target = &resources.handles;
        let transfer = source.prepare_transfer(target, &[request])?;
        let handle = transfer.handles().next().unwrap();
        transfer.commit()?;
        Ok(handle)
    }

    pub fn try_send(
        &self,
        channel: Handle,
        message: Message,
        transfers: &[TransferRequest],
    ) -> Result<(), TaskError> {
        self.send_with(channel, message, transfers, WaitMode::NonBlocking)
    }

    pub fn send(
        &self,
        channel: Handle,
        message: Message,
        transfers: &[TransferRequest],
    ) -> Result<(), TaskError> {
        self.send_with(channel, message, transfers, WaitMode::Blocking)
    }

    pub fn send_until(
        &self,
        channel: Handle,
        message: Message,
        transfers: &[TransferRequest],
        deadline_ns: u64,
    ) -> Result<(), TaskError> {
        self.send_with(channel, message, transfers, WaitMode::Deadline(deadline_ns))
    }

    pub fn try_receive(
        &self,
        channel: Handle,
        handle_capacity: usize,
    ) -> Result<ReceivedMessage, TaskError> {
        self.receive_with(channel, handle_capacity, WaitMode::NonBlocking)
    }

    pub fn receive(
        &self,
        channel: Handle,
        handle_capacity: usize,
    ) -> Result<ReceivedMessage, TaskError> {
        self.receive_with(channel, handle_capacity, WaitMode::Blocking)
    }

    pub fn receive_until(
        &self,
        channel: Handle,
        handle_capacity: usize,
        deadline_ns: u64,
    ) -> Result<ReceivedMessage, TaskError> {
        self.receive_with(channel, handle_capacity, WaitMode::Deadline(deadline_ns))
    }

    pub fn call(
        &self,
        channel: Handle,
        message: Message,
        transfers: &[TransferRequest],
    ) -> Result<ReceivedMessage, TaskError> {
        self.call_with(channel, message, transfers, None)
    }

    pub fn call_until(
        &self,
        channel: Handle,
        message: Message,
        transfers: &[TransferRequest],
        deadline_ns: u64,
    ) -> Result<ReceivedMessage, TaskError> {
        self.call_with(
            channel,
            message,
            transfers,
            (deadline_ns != u64::MAX).then_some(deadline_ns),
        )
    }

    fn call_with(
        &self,
        handle: Handle,
        message: Message,
        transfers: &[TransferRequest],
        deadline: Option<u64>,
    ) -> Result<ReceivedMessage, TaskError> {
        self.call_deliver(handle, message, transfers, deadline, &mut false, |_| Ok(()))
    }

    pub(crate) fn call_deliver(
        &self,
        handle: Handle,
        message: Message,
        transfers: &[TransferRequest],
        deadline: Option<u64>,
        committed: &mut bool,
        deliver: impl FnOnce(&ReceivedMessage) -> Result<(), TaskError>,
    ) -> Result<ReceivedMessage, TaskError> {
        let channel = self.channel(handle, Rights::READ | Rights::WRITE)?;
        {
            let _interrupts = InterruptGuard::new();
            TASKS
                .lock()
                .find(self)?
                .handles
                .validate_transfer::<{ MESSAGE_HANDLES - 1 }>(transfers)?;
        }
        let mut call = Call::new(self, deadline)?;
        let mode = deadline.map_or(WaitMode::Blocking, WaitMode::Deadline);
        channel.send_with(message, mode, true, || {
            let registry = TASKS.lock();
            let table = &registry.find(self)?.handles;
            table.get(handle, ObjectType::Channel, Rights::READ | Rights::WRITE)?;
            if let Some(deadline) = deadline
                && hardware::timer().now_ns().map_err(ThreadError::from)? >= deadline
            {
                return Err(ChannelError::TimedOut.into());
            }
            let reply = call.attach()?;
            table.transfer_out::<{ MESSAGE_HANDLES - 1 }, TaskError>(
                transfers,
                Some(super::HandleEntry::reply(reply)),
                |objects| channel.check_transfer(objects).map_err(Into::into),
            )
        })?;
        *committed = true;
        call.wait_with(deliver)
    }

    pub fn reply(
        &self,
        handle: Handle,
        message: Message,
        transfers: &[TransferRequest],
    ) -> Result<(), TaskError> {
        let _interrupts = InterruptGuard::new();
        let ObjectReference::Reply(reply) = self.get(handle, ObjectType::Reply, Rights::WRITE)?
        else {
            unreachable!("invalid reply handle type");
        };
        let target = reply.destination()?;
        let registry = TASKS.lock();
        let source = &registry.find(self)?.handles;
        let target = &registry.find(&target)?.handles;
        source.reply_to(handle, &reply, target, message, transfers)
    }

    pub(crate) fn reserve_handles(&self, count: usize) -> Result<ReservedHandles, TaskError> {
        let _interrupts = InterruptGuard::new();
        Ok(ReservedHandles {
            task: self.clone(),
            handles: TASKS.lock().find(self)?.handles.reserve_handles(count)?,
        })
    }

    pub(crate) fn reserve_reply_handles(&self) -> Result<ReservedHandles, TaskError> {
        let _interrupts = InterruptGuard::new();
        let registry = TASKS.lock();
        Ok(ReservedHandles {
            task: self.clone(),
            handles: registry.find(self)?.handles.reserve_reply_handles()?,
        })
    }

    pub fn try_wait_many(&self, items: &[WaitItem]) -> Result<WaitManyResult, TaskError> {
        self.wait_many_with(items, WaitMode::NonBlocking)
    }

    pub fn wait_many(&self, items: &[WaitItem]) -> Result<WaitManyResult, TaskError> {
        self.wait_many_with(items, WaitMode::Blocking)
    }

    pub fn wait_many_until(
        &self,
        items: &[WaitItem],
        deadline_ns: u64,
    ) -> Result<WaitManyResult, TaskError> {
        self.wait_many_with(items, WaitMode::Deadline(deadline_ns))
    }

    fn wait_many_with(
        &self,
        items: &[WaitItem],
        mode: WaitMode,
    ) -> Result<WaitManyResult, TaskError> {
        Ok(crate::ipc::wait::wait_many(self, items, mode)?)
    }

    pub(crate) fn send_with(
        &self,
        handle: Handle,
        message: Message,
        transfers: &[TransferRequest],
        mode: WaitMode,
    ) -> Result<(), TaskError> {
        let channel = self.channel(handle, Rights::WRITE)?;
        {
            let _interrupts = InterruptGuard::new();
            TASKS
                .lock()
                .find(self)?
                .handles
                .validate_transfer::<MESSAGE_HANDLES>(transfers)?;
        }
        channel.send_with(message, mode, false, || {
            let registry = TASKS.lock();
            let table = &registry.find(self)?.handles;
            table.get(handle, ObjectType::Channel, Rights::WRITE)?;
            table.transfer_out::<MESSAGE_HANDLES, _>(transfers, None, |objects| {
                channel.check_transfer(objects).map_err(Into::into)
            })
        })
    }

    fn receive_with(
        &self,
        handle: Handle,
        capacity: usize,
        mode: WaitMode,
    ) -> Result<ReceivedMessage, TaskError> {
        let channel = self.channel(handle, Rights::READ)?;
        channel.receive_with(mode, capacity, |_, _, entries| {
            let registry = TASKS.lock();
            let table = &registry.find(self)?.handles;
            table.get(handle, ObjectType::Channel, Rights::READ)?;
            Ok(table.install(entries)?)
        })
    }

    pub(crate) fn receive_deliver(
        &self,
        handle: Handle,
        capacity: usize,
        mode: WaitMode,
        mut deliver: impl FnMut(&ReceivedMessage, &[super::HandleEntry]) -> Result<(), TaskError>,
    ) -> Result<ReceivedMessage, TaskError> {
        self.channel(handle, Rights::READ)?.receive_with(
            mode,
            capacity,
            |message, has_reply, entries| {
                self.get(handle, ObjectType::Channel, Rights::READ)?;
                let mut reservation = self.reserve_handles(entries.len())?;
                let response =
                    ReceivedMessage::new(*message, reservation.handles.clone(), has_reply);
                deliver(&response, entries)?;
                reservation.publish_received(entries)?;
                Ok(response.handles().iter().copied().collect())
            },
        )
    }

    fn channel(&self, handle: Handle, rights: Rights) -> Result<ChannelReference, TaskError> {
        let ObjectReference::Channel(channel) = self.get(handle, ObjectType::Channel, rights)?
        else {
            unreachable!("invalid channel handle type");
        };
        Ok(channel)
    }

    pub fn allocate_frames(&self, frames: u64) -> Result<FrameAllocation, TaskError> {
        let _interrupts = InterruptGuard::new();
        TASKS.lock().find(self)?;
        Ok(memory::manager().allocate(
            memory::FrameOwner::User(self.0.id),
            memory::FramePurpose::Data,
            frames,
        )?)
    }

    pub fn map(
        &self,
        allocation: &FrameAllocation,
        frames: Range<u64>,
        options: MappingOptions,
    ) -> Result<VirtRange, TaskError> {
        let _interrupts = InterruptGuard::new();
        let registry = TASKS.lock();
        Ok(registry
            .find(self)?
            .space
            .map(allocation, frames, options)?)
    }

    pub(crate) fn pin_user_memory(
        &self,
        address: usize,
        length: usize,
        write: bool,
    ) -> Result<crate::memory::MemoryBuffer, crate::memory::UserMemoryError> {
        let _interrupts = InterruptGuard::new();
        let registry = TASKS.lock();
        let resources = registry
            .find(self)
            .map_err(|_| crate::memory::UserMemoryError(MemoryError::NotMapped))?;
        resources.space.pin_user_memory(address, length, write)
    }

    pub fn map_at(
        &self,
        address: usize,
        allocation: &FrameAllocation,
        frames: Range<u64>,
        options: MappingOptions,
    ) -> Result<VirtRange, TaskError> {
        let _interrupts = InterruptGuard::new();
        Ok(TASKS
            .lock()
            .find(self)?
            .space
            .map_at(address, allocation, frames, options)?)
    }

    pub fn protect(&self, range: VirtRange, permissions: PagePermissions) -> Result<(), TaskError> {
        let _interrupts = InterruptGuard::new();
        Ok(TASKS.lock().find(self)?.space.protect(range, permissions)?)
    }

    pub fn reserve(
        &self,
        length: usize,
        alignment: usize,
        address: Option<usize>,
    ) -> Result<VirtRange, TaskError> {
        if alignment < crate::hardware::mmu::PAGE_SIZE
            || !alignment.is_power_of_two()
            || address.is_some_and(|address| !address.is_multiple_of(alignment))
        {
            return Err(MemoryError::InvalidRequest.into());
        }
        let _interrupts = InterruptGuard::new();
        let registry = TASKS.lock();
        let space = &registry.find(self)?.space;
        if let Some(start) = address {
            let range = VirtRange {
                start,
                end: start
                    .checked_add(length)
                    .ok_or(MemoryError::InvalidRequest)?,
            };
            space.reserve_at(range)?;
            Ok(range)
        } else {
            Ok(space.reserve(length, alignment)?)
        }
    }

    pub fn unreserve(&self, range: VirtRange) -> Result<(), TaskError> {
        let _interrupts = InterruptGuard::new();
        Ok(TASKS.lock().find(self)?.space.unreserve(range)?)
    }

    pub fn unmap(&self, range: VirtRange) -> Result<(), TaskError> {
        let _interrupts = InterruptGuard::new();
        let registry = TASKS.lock();
        Ok(registry.find(self)?.space.unmap(range)?)
    }

    pub fn query(&self, address: usize) -> Result<Option<MappingInfo>, TaskError> {
        let _interrupts = InterruptGuard::new();
        let registry = TASKS.lock();
        Ok(registry.find(self)?.space.query(address)?)
    }

    pub fn terminate(&self, status: usize) {
        let _interrupts = InterruptGuard::new();
        TASKS
            .lock()
            .signal(|resources| resources.task.same(self), status);
        scheduler::reap_terminated();
        reclaim_finished();
    }

    pub fn wait(&self) -> Result<usize, TaskError> {
        Ok(thread::wait_for_task(self.clone(), None)?.unwrap())
    }

    pub fn wait_until(&self, deadline_ns: u64) -> Result<Option<usize>, TaskError> {
        Ok(thread::wait_for_task(self.clone(), Some(deadline_ns))?)
    }

    pub fn create_thread(&self, entry: UserEntry) -> Result<Thread, TaskError> {
        let _interrupts = InterruptGuard::new();
        let mut registry = TASKS.lock();
        registry.find(self)?.validate_entry(entry)?;
        let thread = Thread::new_user(self.clone(), entry)?;
        let resources = registry.find_mut(self)?;
        thread.set_task_next(resources.threads.take());
        resources.threads = Some(thread.clone());
        Ok(thread)
    }

    pub(crate) fn start_thread(&self, thread: &Thread) -> Result<(), ThreadError> {
        let _interrupts = InterruptGuard::new();
        let mut registry = TASKS.lock();
        let resources = registry.find(self).map_err(|_| ThreadError::InvalidState)?;
        if thread.state() != ThreadState::Created {
            return Err(ThreadError::InvalidState);
        }
        resources.validate_entry(thread.initial_entry().ok_or(ThreadError::InvalidState)?)?;
        scheduler::start(thread)?;
        if self.state() == TaskState::Created {
            self.0.data.lock().state = TaskState::Active;
            registry.find_mut(self).unwrap().creator = None;
        }
        Ok(())
    }

    pub(crate) fn activate(&self) -> Result<(), ThreadError> {
        let mut registry = TASKS.lock();
        let resources = registry
            .find_mut(self)
            .map_err(|_| ThreadError::InvalidState)?;
        resources.space.activate()?;
        Ok(())
    }

    pub(crate) fn same(&self, other: &Self) -> bool {
        HeapArc::ptr_eq(&self.0, &other.0)
    }

    pub(crate) fn with_exit_waiters<T>(&self, action: impl FnOnce(&mut Option<Thread>) -> T) -> T {
        action(&mut self.0.data.lock().exit_waiters)
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

    pub(crate) fn finish_exit(&self, status: usize) {
        self.0.data.lock().state = TaskState::Dead(status);
    }
}

impl ReservedHandles {
    fn publish_received(
        &mut self,
        entries: &mut alloc::vec::Vec<super::HandleEntry>,
    ) -> Result<(), TaskError> {
        let _interrupts = InterruptGuard::new();
        TASKS
            .lock()
            .find(&self.task)?
            .handles
            .publish_received(&self.handles, entries)?;
        self.handles.clear();
        Ok(())
    }

    pub(crate) fn publish<const N: usize>(
        &mut self,
        entries: &mut ArrayVec<super::HandleEntry, N>,
    ) -> Result<(), TaskError> {
        let _interrupts = InterruptGuard::new();
        TASKS
            .lock()
            .find(&self.task)?
            .handles
            .publish_reserved(&self.handles, entries)?;
        self.handles.clear();
        Ok(())
    }
}

impl Drop for ReservedHandles {
    fn drop(&mut self) {
        let _interrupts = InterruptGuard::new();
        let entries = {
            let mut registry = TASKS.lock();
            registry
                .find_mut(&self.task)
                .map(|resources| resources.handles.release_reply_handles(&self.handles))
        };
        drop(entries);
    }
}

impl TaskRegistry {
    fn create(&mut self, creator: Creator) -> Result<Task, TaskError> {
        // 子 Task 排在创建者之后，使终止可按祖先顺序单次遍历。
        let position = match &creator {
            Creator::Thread(_) => &mut self.first,
            Creator::Task(parent) => {
                self.find(parent)?;
                &mut self.find_mut(parent)?.next
            }
        };
        let space = AddressSpace::new()?;
        let id = NEXT_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| MemoryError::NoMemory)?;
        let task = Task(memory::manager().allocate_arc(TaskRecord {
            id,
            data: Mutex::new(TaskData {
                state: TaskState::Created,
                exit_waiters: None,
                observers: Observers::new(),
            }),
        })?);
        let mut resources = HeapBox::try_new(TaskResources {
            task: task.clone(),
            creator: Some(creator),
            space,
            handles: HandleTable::new(),
            threads: None,
            next: None,
        })
        .map_err(|_| MemoryError::NoMemory)?;
        resources.next = position.take();
        *position = Some(resources);
        Ok(task)
    }

    fn find(&self, task: &Task) -> Result<&TaskResources, TaskError> {
        if !matches!(task.state(), TaskState::Created | TaskState::Active) {
            return Err(TaskError::InvalidState);
        }
        let mut cursor = self.first.as_deref();
        while let Some(resources) = cursor {
            if resources.task.same(task) {
                return Ok(resources);
            }
            cursor = resources.next.as_deref();
        }
        Err(TaskError::InvalidState)
    }

    fn find_mut(&mut self, task: &Task) -> Result<&mut TaskResources, TaskError> {
        let mut cursor = self.first.as_deref_mut();
        while let Some(resources) = cursor {
            if resources.task.same(task) {
                return Ok(resources);
            }
            cursor = resources.next.as_deref_mut();
        }
        Err(TaskError::InvalidState)
    }

    fn signal(&mut self, matches: impl Fn(&TaskResources) -> bool, status: usize) {
        let mut cursor = self.first.as_deref();
        while let Some(resources) = cursor {
            let parent_dying = matches!(&resources.creator, Some(Creator::Task(task)) if matches!(task.state(), TaskState::Dying(_)));
            if (matches(resources) || parent_dying)
                && !matches!(resources.task.state(), TaskState::Dying(_))
            {
                resources.task.0.data.lock().state = TaskState::Dying(status);
                let mut thread = resources.threads.clone();
                while let Some(current) = thread {
                    thread = current.task_next();
                    scheduler::terminate(&current, status);
                }
            }
            cursor = resources.next.as_deref();
        }
    }

    fn detach_finished(&mut self) -> Option<HeapBox<TaskResources>> {
        let mut cleanup = None;
        let mut cursor = &mut self.first;
        while let Some(resources) = cursor.as_ref() {
            if matches!(resources.task.state(), TaskState::Dying(_)) && resources.threads.is_none()
            {
                let mut resources = cursor.take().unwrap();
                *cursor = resources.next.take();
                resources.next = cleanup;
                cleanup = Some(resources);
            } else {
                cursor = &mut cursor.as_mut().unwrap().next;
            }
        }
        cleanup
    }
}

pub(crate) fn thread_exited(thread: &Thread, status: usize) {
    let _interrupts = InterruptGuard::new();
    let mut registry = TASKS.lock();
    if let Some(task) = thread.task() {
        let resources = registry.find_mut(&task).expect("missing thread task");
        resources.remove_thread(thread);
        if resources.threads.is_none() && task.state() == TaskState::Active {
            registry.signal(|resources| resources.task.same(&task), status);
        }
    } else {
        registry.signal(|resources| matches!(&resources.creator, Some(Creator::Thread(creator)) if creator.same(thread)), status);
    }
}

pub(crate) fn reclaim_finished() {
    let cleanup = TASKS.lock().detach_finished();
    reclaim(cleanup);
}

impl TaskResources {
    fn validate_entry(&self, entry: UserEntry) -> Result<(), ThreadError> {
        let permissions = |address| -> Result<Option<PagePermissions>, ThreadError> {
            Ok(self
                .space
                .query(address)?
                .and_then(|info| info.mapping)
                .map(|mapping| mapping.attributes.permissions))
        };
        if !matches!(
            permissions(entry.pc)?,
            Some(PagePermissions::ReadExecute | PagePermissions::ExecuteOnly)
        ) {
            return Err(ExecutionError::InvalidEntry.into());
        }
        let stack = entry
            .stack_pointer
            .checked_sub(1)
            .ok_or(ExecutionError::InvalidStack)?;
        if !entry.stack_pointer.is_multiple_of(16)
            || permissions(stack)? != Some(PagePermissions::ReadWrite)
        {
            return Err(ExecutionError::InvalidStack.into());
        }
        if entry.thread_pointer != 0
            && !matches!(
                permissions(entry.thread_pointer)?,
                Some(PagePermissions::ReadOnly | PagePermissions::ReadWrite)
            )
        {
            return Err(ExecutionError::InvalidEntry.into());
        }
        Ok(())
    }

    fn remove_thread(&mut self, target: &Thread) {
        let mut previous: Option<Thread> = None;
        let mut cursor = self.threads.clone();
        while let Some(thread) = cursor {
            let next = thread.task_next();
            if thread.same(target) {
                if let Some(previous) = previous {
                    previous.set_task_next(next);
                } else {
                    self.threads = next;
                }
                thread.set_task_next(None);
                return;
            }
            previous = Some(thread);
            cursor = next;
        }
        panic!("missing task thread");
    }
}

fn reclaim(mut cleanup: Option<HeapBox<TaskResources>>) {
    while let Some(mut resources) = cleanup {
        cleanup = resources.next.take();
        let task = resources.task.clone();
        let TaskState::Dying(status) = task.state() else {
            unreachable!("invalid task cleanup state");
        };
        drop(resources);
        thread::finish_task_exit(&task, status);
    }
}
