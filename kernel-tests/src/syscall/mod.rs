// SPDX-License-Identifier: MPL-2.0

use core::sync::atomic::{AtomicBool, Ordering};
use kernel::{
    hardware::{
        execution::UserEntry,
        mmu::{PAGE_SIZE, PagePermissions},
    },
    memory::{self, FrameAccess, FrameOwner, FramePurpose, MappingOptions, ReferenceKind},
    task::{Object, ObjectType, Task, TaskState},
};
use spin::Once;

mod program;
pub use program::{run as run_program, run_device as run_device_program};

static ACTIVE: AtomicBool = AtomicBool::new(false);
static CODE: Once<[u64; PAGE_SIZE / 8]> = Once::new();

pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

pub fn run(code: &[u64]) {
    CODE.call_once(|| code.try_into().unwrap());
    let manager = memory::manager();
    manager.reclaim_heap();
    let before = manager.stats();
    let heap = manager.heap_stats();
    ACTIVE.store(true, Ordering::Relaxed);
    crate::run_thread(scenario);
    ACTIVE.store(false, Ordering::Relaxed);
    manager.reclaim_heap();
    assert_eq!(manager.stats(), before);
    assert_eq!(manager.heap_stats(), heap);
}

fn scenario(_: usize) -> usize {
    let manager = memory::manager();
    let task = Task::new().unwrap();
    let code = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 1)
        .unwrap();
    code.access(
        code.frames().next().unwrap(),
        ReferenceKind::Temporary,
        FrameAccess::Write,
    )
    .unwrap()
    .write(0, CODE.get().unwrap())
    .unwrap();
    let text = task
        .map(
            &code,
            0..1,
            MappingOptions::new(PagePermissions::ReadExecute),
        )
        .unwrap();
    let stack = manager
        .allocate(FrameOwner::User(1), FramePurpose::Stack, 4)
        .unwrap();
    let range = task
        .map(
            &stack,
            0..4,
            MappingOptions::new(PagePermissions::ReadWrite),
        )
        .unwrap();
    let authority = task
        .insert(
            Object::Task(task.clone()),
            ObjectType::Task.supported_rights(),
        )
        .unwrap();
    let user = task
        .create_thread(UserEntry {
            pc: text.start,
            stack_pointer: range.end,
            thread_pointer: range.start,
            argument: authority.raw() as usize,
        })
        .unwrap();
    user.start().unwrap();
    assert_eq!(user.wait().unwrap(), 73);
    assert_eq!(task.state(), TaskState::Dead(73));
    let access = stack
        .access(
            stack.frames().next().unwrap(),
            ReferenceKind::Temporary,
            FrameAccess::Read,
        )
        .unwrap();
    let mut marker = [0];
    access.read(0, &mut marker).unwrap();
    assert_eq!(marker, [73]);
    0
}
