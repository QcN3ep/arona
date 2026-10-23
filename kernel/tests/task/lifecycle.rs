// SPDX-License-Identifier: MPL-2.0

use spin::Mutex;

use kernel::{
    hardware::{self, mmu::PagePermissions},
    memory::{self, FrameOwner, FramePurpose, MappingOptions, MemoryError},
    task::{
        Handle, HandleError, Object, ObjectType, Rights, Task, TaskError, TaskState,
        TransferRequest,
    },
    thread::{self, Thread, ThreadState, scheduler},
};

static TASK: Mutex<Option<Task>> = Mutex::new(None);
static CHILD: Mutex<Option<Task>> = Mutex::new(None);
static PUBLISHED: Mutex<Option<Handle>> = Mutex::new(None);

pub fn run() {
    let manager = memory::manager();
    manager.reclaim_heap();
    let heap = manager.heap_stats();
    let frames = manager.stats();
    assert!(matches!(Task::new(), Err(TaskError::InvalidState)));
    let owner = Thread::new(owner_lifecycle, 0).unwrap();
    owner.start().unwrap();
    scheduler::run_ready().unwrap();
    assert_eq!(owner.state(), ThreadState::Dead(73));
    let parent = TASK.lock().take().unwrap();
    let child = CHILD.lock().take().unwrap();
    assert_eq!(parent.state(), TaskState::Dead(73));
    assert_eq!(child.state(), TaskState::Dead(73));
    assert_eq!(manager.heap_stats().allocations, heap.allocations + 3);
    assert!(matches!(
        parent.create_child(),
        Err(TaskError::InvalidState)
    ));
    assert!(matches!(
        parent.query(0x1_0000),
        Err(TaskError::InvalidState)
    ));
    assert_eq!(
        parent.close(PUBLISHED.lock().take().unwrap()),
        Err(TaskError::InvalidState)
    );
    parent.terminate(99);
    assert_eq!(parent.state(), TaskState::Dead(73));
    drop(parent);
    drop(child);
    drop(owner);
    waiting_lifecycle();
    deep_cleanup();
    allocation_failure();
    manager.reclaim_heap();
    assert_eq!(manager.heap_stats(), heap);
    assert_eq!(manager.stats(), frames);
}

fn owner_lifecycle(_: usize) -> usize {
    let parent = Task::new().unwrap();
    let child = parent.create_child().unwrap();
    assert_eq!(parent.state(), TaskState::Created);
    let self_handle = parent
        .insert(
            Object::Task(parent.clone()),
            Rights::WAIT | Rights::TRANSFER | Rights::DUPLICATE,
        )
        .unwrap();
    assert!(matches!(
        parent.get(self_handle, ObjectType::Thread, Rights::START),
        Err(TaskError::Handle(HandleError::WrongType))
    ));
    let duplicate = parent.duplicate(self_handle, Rights::WAIT).unwrap();
    parent.close(duplicate).unwrap();
    let installed = child
        .install_handle(
            &parent,
            TransferRequest {
                handle: self_handle,
                rights: Rights::WAIT,
            },
        )
        .unwrap();
    assert_eq!(
        parent.close(self_handle),
        Err(TaskError::Handle(HandleError::BadHandle))
    );
    assert!(matches!(
        child.get(installed, ObjectType::Task, Rights::WAIT),
        Ok(Object::Task(_))
    ));
    let allocation = memory::manager()
        .allocate(FrameOwner::User(1), FramePurpose::Data, 2)
        .unwrap();
    let range = parent
        .map(
            &allocation,
            0..2,
            MappingOptions::new(PagePermissions::ReadWrite),
        )
        .unwrap();
    assert!(
        parent
            .query(range.start)
            .unwrap()
            .unwrap()
            .mapping
            .unwrap()
            .attributes
            .user_accessible
    );
    parent.unmap(range).unwrap();
    parent
        .map(
            &allocation,
            0..2,
            MappingOptions::new(PagePermissions::ReadWrite),
        )
        .unwrap();
    child
        .insert(Object::Task(child.clone()), Rights::WAIT)
        .unwrap();
    *TASK.lock() = Some(parent);
    *CHILD.lock() = Some(child);
    *PUBLISHED.lock() = Some(installed);
    73
}

fn waiting_lifecycle() {
    let owner = Thread::new(
        |_| {
            let task = Task::new().unwrap();
            *TASK.lock() = Some(task.clone());
            thread::yield_now().unwrap();
            assert_eq!(task.wait_until(0), Ok(None));
            assert_eq!(
                task.wait_until(hardware::timer().now_ns().unwrap() + 1_000_000),
                Ok(None)
            );
            assert_eq!(task.state(), TaskState::Created);
            task.terminate(42);
            task.terminate(99);
            assert_eq!(task.wait_until(0), Ok(Some(42)));
            assert_eq!(task.wait(), Ok(42));
            0
        },
        0,
    )
    .unwrap();
    let waiter = Thread::new(
        |_| {
            let task = TASK.lock().as_ref().unwrap().clone();
            assert_eq!(task.wait(), Ok(42));
            assert_eq!(task.state(), TaskState::Dead(42));
            19
        },
        0,
    )
    .unwrap();
    owner.start().unwrap();
    waiter.start().unwrap();
    scheduler::run_ready().unwrap();
    assert_eq!(owner.state(), ThreadState::Blocked);
    assert_eq!(waiter.state(), ThreadState::Blocked);
    while owner.state() != ThreadState::Dead(0) {
        hardware::cpu().idle_until_interrupt();
        scheduler::run_ready().unwrap();
    }
    assert_eq!(waiter.state(), ThreadState::Dead(19));
    *TASK.lock() = None;
}

fn deep_cleanup() {
    Thread::new(
        |_| {
            let root = Task::new().unwrap();
            let mut leaf = root.clone();
            for _ in 0..256 {
                leaf = leaf.create_child().unwrap();
            }
            leaf.insert(Object::Task(root.clone()), Rights::WAIT)
                .unwrap();
            root.terminate(91);
            assert_eq!(root.state(), TaskState::Dead(91));
            assert_eq!(leaf.state(), TaskState::Dead(91));
            0
        },
        0,
    )
    .unwrap()
    .start()
    .unwrap();
    scheduler::run_ready().unwrap();
}

fn allocation_failure() {
    Thread::new(
        |_| {
            let manager = memory::manager();
            let parent = Task::new().unwrap();
            let object = Object::FrameAllocation(
                manager
                    .allocate_arc(
                        manager
                            .allocate(FrameOwner::Kernel, FramePurpose::Data, 1)
                            .unwrap(),
                    )
                    .unwrap(),
            );
            let handle = parent
                .insert(object, Rights::TRANSFER | Rights::READ)
                .unwrap();
            let child = parent.create_child().unwrap();
            manager.reclaim_heap();
            let free = manager.stats().kernel.free_frames;
            let occupied = manager
                .allocate(FrameOwner::Kernel, FramePurpose::Data, free)
                .unwrap();
            let heap = manager.heap_stats();
            assert!(matches!(
                parent.create_child(),
                Err(TaskError::Memory(MemoryError::NoMemory))
            ));
            assert!(
                parent
                    .get(handle, ObjectType::FrameAllocation, Rights::READ)
                    .is_ok()
            );
            assert_eq!(manager.heap_stats(), heap);
            child.terminate(17);
            assert_eq!(child.state(), TaskState::Dead(17));
            drop(occupied);
            let destination = parent.create_child().unwrap();
            let installed = destination
                .install_handle(
                    &parent,
                    TransferRequest {
                        handle,
                        rights: Rights::READ,
                    },
                )
                .unwrap();
            assert!(
                destination
                    .get(installed, ObjectType::FrameAllocation, Rights::READ)
                    .is_ok()
            );
            parent.terminate(29);
            assert_eq!(destination.state(), TaskState::Dead(29));
            assert_eq!(child.state(), TaskState::Dead(17));
            0
        },
        0,
    )
    .unwrap()
    .start()
    .unwrap();
    scheduler::run_ready().unwrap();
    assert!(thread::current().is_none());
    let task = TASK.lock().take();
    assert!(task.is_none());
}
