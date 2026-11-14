// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use core::sync::atomic::{AtomicBool, Ordering};
use kernel::{
    hardware::{
        execution::UserEntry,
        mmu::{PAGE_SIZE, PagePermissions},
    },
    ipc::WaitItem,
    memory::{self, FrameAccess, FrameOwner, FramePurpose, MappingOptions, ReferenceKind},
    task::{Handle, HandleError, Object, Rights, Task, TaskError, TaskState},
    thread::{self, ThreadState},
    trap::port::{ForwardError, TrapPort},
};
use spin::Once;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static CODE: Once<[u64; PAGE_SIZE / 8]> = Once::new();
static RETURN: Once<fn(&mut kernel::hardware::execution::UserState, [usize; 3])> = Once::new();

pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

pub fn run<P: crate::Platform>() {
    CODE.call_once(|| P::user_code().try_into().unwrap());
    RETURN.call_once(|| P::return_values);
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
    let owner = Task::new().unwrap();
    let port = owner.create_trap_port().unwrap();
    let handle = owner
        .insert(Object::TrapPort(port.clone()), Rights::READ | Rights::WAIT)
        .unwrap();
    let child = owner.create_child().unwrap();
    child.bind_trap(&port).unwrap();
    assert_eq!(child.bind_trap(&port), Err(ForwardError::InvalidState));
    let (user, pc) = create_user(&child);
    user.start().unwrap();
    user.interrupt().unwrap();
    assert_eq!(user.state(), ThreadState::Blocked);
    let interrupted = port.try_receive().unwrap();
    assert_eq!(interrupted.info().kind, abi::trap::TrapInfo::INTERRUPTED);
    let continuation = interrupted.commit();
    let initial = continuation.reference().user_state().unwrap();
    assert_eq!(initial.pc, pc);
    continuation.reference().resume(&initial).unwrap();
    drop(continuation);
    forward_syscalls(&owner, &user, &port, handle);
    owner
        .wait_many(&[WaitItem {
            handle,
            condition_mask: 1,
        }])
        .unwrap();
    drop(port.try_receive().unwrap().commit());
    assert_eq!(user.wait().unwrap(), usize::MAX);
    assert_eq!(child.state(), TaskState::Dead(usize::MAX));
    owner.close(handle).unwrap();
    drop(port);
    close_ports(&owner);
    0
}

fn create_user(child: &Task) -> (kernel::thread::Thread, usize) {
    let code = child.allocate_frames(1).unwrap();
    code.access(
        code.frames().next().unwrap(),
        ReferenceKind::Temporary,
        FrameAccess::Write,
    )
    .unwrap()
    .write(0, CODE.get().unwrap())
    .unwrap();
    let text = child
        .map(
            &code,
            0..1,
            MappingOptions::new(PagePermissions::ReadExecute),
        )
        .unwrap();
    let stack = memory::manager()
        .allocate(FrameOwner::User(1), FramePurpose::Stack, 2)
        .unwrap();
    stack
        .access(
            stack.frames().next().unwrap(),
            ReferenceKind::Temporary,
            FrameAccess::Write,
        )
        .unwrap()
        .write(0, &[2])
        .unwrap();
    let range = child
        .map(
            &stack,
            0..2,
            MappingOptions::new(PagePermissions::ReadWrite),
        )
        .unwrap();
    let user = child
        .create_thread(UserEntry {
            pc: text.start,
            stack_pointer: range.end,
            thread_pointer: range.start,
            argument: 73,
        })
        .unwrap();
    (user, text.start)
}

fn forward_syscalls(owner: &Task, user: &kernel::thread::Thread, port: &TrapPort, handle: Handle) {
    for remaining in [2, 1] {
        let ready = owner
            .wait_many(&[WaitItem {
                handle,
                condition_mask: 1,
            }])
            .unwrap();
        assert_eq!(ready.index, 0);
        assert_eq!(user.state(), ThreadState::Blocked);
        let delivery = port.try_receive().unwrap();
        let info = delivery.info();
        assert_eq!(info.thread, user.id());
        assert_eq!(info.kind, abi::trap::TrapInfo::SYSCALL);
        drop(delivery);
        let continuation = port.try_receive().unwrap().commit();
        let reference = continuation.reference();
        let mut state = reference.user_state().unwrap();
        let mut invalid = state.clone();
        invalid.pc = usize::MAX;
        assert!(reference.resume(&invalid).is_err());
        assert_eq!(reference.user_state().unwrap(), state);
        state.pc = info.successor as usize;
        RETURN.get().unwrap()(&mut state, [0, 73, remaining]);
        reference.resume(&state).unwrap();
        assert_eq!(reference.resume(&state), Err(ForwardError::InvalidState));
        drop(continuation);
        thread::yield_now().unwrap();
    }
}

fn close_ports(owner: &Task) {
    for rights in [Rights::READ, Rights::WRITE] {
        let child = Task::new().unwrap();
        let port = owner.create_trap_port().unwrap();
        child.bind_trap(&port).unwrap();
        let result = owner.insert(Object::TrapPort(port), rights);
        if rights == Rights::READ {
            owner.close(result.unwrap()).unwrap();
        } else {
            assert_eq!(result, Err(TaskError::Handle(HandleError::InvalidArgument)));
        }
        assert_eq!(child.state(), TaskState::Dead(usize::MAX));
    }
    let mut other = owner.create_child().unwrap();
    let exclusive = owner.create_trap_port().unwrap();
    other.bind_trap(&exclusive).unwrap();
    drop(exclusive);
    assert_eq!(other.state(), TaskState::Dead(usize::MAX));
    let new_owner = Task::new().unwrap();
    let final_port = new_owner.create_trap_port().unwrap();
    other = Task::new().unwrap();
    let closed = other.create_trap_port().unwrap();
    drop(closed);
    other.bind_trap(&final_port).unwrap();
    drop(final_port);
    assert_eq!(other.state(), TaskState::Dead(usize::MAX));
    new_owner.terminate(0);
    let supervised = Task::new().unwrap();
    let inherited = owner.create_trap_port().unwrap();
    supervised.bind_trap(&inherited).unwrap();
    owner.terminate(0);
    assert_eq!(supervised.state(), TaskState::Dead(usize::MAX));
    assert!(matches!(inherited.try_receive(), Err(ForwardError::Closed)));
}
