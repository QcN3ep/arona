// SPDX-License-Identifier: MPL-2.0

//! Task 的创建归属、运行资源回收与退出等待。

use core::ops::Range;
use spin::Mutex;

use super::{Handle, HandleError, HandleTable, Object, ObjectType, Rights, TransferRequest};
use crate::{
    hardware::{self, cpu::InterruptState},
    memory::{
        self, AddressSpace, FrameAllocation, HeapArc, HeapBox, MappingInfo, MappingOptions,
        MemoryError, VirtRange,
    },
    thread::{self, Thread, ThreadError},
};

static TASKS: Mutex<TaskRegistry> = Mutex::new(TaskRegistry { first: None });

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskState {
    Created,
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
    creator: Creator,
    space: AddressSpace,
    handles: HandleTable,
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
        TASKS.lock().create(Creator::Thread(creator))
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
        let source = &registry.find(source)?.handles;
        let target = &registry.find(self)?.handles;
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
        let cleanup = TASKS
            .lock()
            .detach(|resources| resources.task.same(self), status);
        reclaim(cleanup);
    }

    pub fn wait(&self) -> Result<usize, TaskError> {
        Ok(thread::wait_for_task(self.clone(), None)?.unwrap())
    }

    pub fn wait_until(&self, deadline_ns: u64) -> Result<Option<usize>, TaskError> {
        Ok(thread::wait_for_task(self.clone(), Some(deadline_ns))?)
    }

    fn same(&self, other: &Self) -> bool {
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
            Creator::Task(parent) => &mut self.find_mut(parent)?.next,
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
            creator,
            space,
            handles: HandleTable::new(),
            next: None,
        })
        .map_err(|_| MemoryError::NoMemory)?;
        resources.next = position.take();
        *position = Some(resources);
        Ok(task)
    }

    fn find(&self, task: &Task) -> Result<&TaskResources, TaskError> {
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

    fn detach(
        &mut self,
        matches: impl Fn(&TaskResources) -> bool,
        status: usize,
    ) -> Option<HeapBox<TaskResources>> {
        let mut cleanup = None;
        let mut cursor = &mut self.first;
        while let Some(resources) = cursor.as_ref() {
            let parent_dying = matches!(&resources.creator, Creator::Task(task) if matches!(task.state(), TaskState::Dying(_)));
            if matches(resources) || parent_dying {
                let mut resources = cursor.take().unwrap();
                *cursor = resources.next.take();
                resources.task.0.data.lock().state = TaskState::Dying(status);
                resources.next = cleanup;
                cleanup = Some(resources);
            } else {
                cursor = &mut cursor.as_mut().unwrap().next;
            }
        }
        cleanup
    }
}

pub(crate) fn creator_exited(thread: &Thread, status: usize) {
    let _interrupts = InterruptGuard::new();
    let cleanup = TASKS.lock().detach(
        |resources| matches!(&resources.creator, Creator::Thread(creator) if creator.same(thread)),
        status,
    );
    reclaim(cleanup);
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
