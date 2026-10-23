// SPDX-License-Identifier: MPL-2.0

//! Task 的创建归属、运行资源回收与退出等待。

use core::ops::Range;
use spin::Mutex;

use super::{Handle, HandleError, HandleTable, Object, ObjectType, Rights, TransferRequest};
use crate::{
    hardware::{
        self,
        cpu::InterruptState,
        execution::{ExecutionError, UserEntry},
        mmu::PagePermissions,
    },
    memory::{
        self, AddressSpace, FrameAllocation, HeapArc, HeapBox, MappingInfo, MappingOptions,
        MemoryError, VirtRange,
    },
    thread::{self, Thread, ThreadError, ThreadState, scheduler},
};

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
    data: Mutex<TaskData>,
}

struct TaskData {
    state: TaskState,
    exit_waiters: Option<Thread>,
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

struct InterruptGuard(InterruptState);

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

    pub fn get(
        &self,
        handle: Handle,
        kind: ObjectType,
        rights: Rights,
    ) -> Result<Object, TaskError> {
        let _interrupts = InterruptGuard::new();
        let registry = TASKS.lock();
        Ok(registry.find(self)?.handles.get(handle, kind, rights)?)
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
        if self.state() != TaskState::Created
            || !matches!(&resources.creator, Some(Creator::Task(parent)) if parent.same(source))
        {
            return Err(TaskError::InvalidState);
        }
        let source = &registry.find(source)?.handles;
        let target = &resources.handles;
        let transfer = source.prepare_transfer(target, &[request])?;
        let handle = transfer.handles().next().unwrap();
        transfer.commit()?;
        Ok(handle)
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

    pub(crate) fn finish_exit(&self, status: usize) {
        self.0.data.lock().state = TaskState::Dead(status);
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
        let task = Task(memory::manager().allocate_arc(TaskRecord {
            data: Mutex::new(TaskData {
                state: TaskState::Created,
                exit_waiters: None,
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
