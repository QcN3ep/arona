// SPDX-License-Identifier: MPL-2.0

use spin::{Mutex, Once};

use kernel::{
    hardware::{
        self,
        execution::{ExecutionError, UserEntry},
        mmu::{PAGE_SIZE, PagePermissions},
    },
    memory::{
        self, FrameAccess, FrameAllocation, FrameOwner, FramePurpose, MappingOptions,
        ReferenceKind, VirtRange,
    },
    task::{Object, Rights, Task, TaskError, TaskState, TransferRequest},
    thread::{self, Thread, ThreadError, ThreadState, Waker, scheduler},
    trap::{Trap, TrapEvent, TrapOutcome},
};

static CODE: Once<[u64; PAGE_SIZE / 8]> = Once::new();
static BOOT: Mutex<Option<Fixture>> = Mutex::new(None);
static OWNER: Mutex<Option<Waker>> = Mutex::new(None);
static WAIT_TARGET: Mutex<Option<Task>> = Mutex::new(None);

struct Fixture {
    task: Task,
    thread: Thread,
    stack: FrameAllocation,
    entry: UserEntry,
}

enum Action {
    Return(TrapOutcome),
    Exit(usize),
    Dispatch,
}

pub fn handle(trap: Trap) -> TrapOutcome {
    match handle_user(trap) {
        Action::Return(outcome) => outcome,
        Action::Exit(status) => thread::exit(status),
        Action::Dispatch => kernel::trap::dispatch(trap),
    }
}

fn handle_user(trap: Trap) -> Action {
    // 用户 Trap 作为测试同步点。
    let TrapEvent::Syscall { arguments, .. } = trap.event else {
        return Action::Dispatch;
    };
    let current = thread::current().unwrap();
    if matches!(current.state(), ThreadState::Dying(_)) {
        return Action::Dispatch;
    }
    if arguments[1] == 0 {
        return Action::Exit(arguments[0]);
    }
    let target = WAIT_TARGET.lock().clone();
    if let Some(target) = target {
        assert_eq!(
            target.wait(),
            Err(TaskError::Thread(ThreadError::Terminated))
        );
        return Action::Dispatch;
    }
    thread::yield_now().unwrap();
    if matches!(current.state(), ThreadState::Dying(_)) {
        Action::Dispatch
    } else {
        Action::Return(TrapOutcome::Return {
            status: 0,
            values: [arguments[0], arguments[1]],
        })
    }
}

pub fn run(code: &[u64], set_floating_enabled: fn(bool)) {
    CODE.call_once(|| code.try_into().unwrap());
    let manager = memory::manager();
    manager.reclaim_heap();
    let heap = manager.heap_stats();
    let frames = manager.stats();
    entry_rollback_and_creator_exit(set_floating_enabled);
    let worker = Thread::new(scenarios, 0).unwrap();
    worker.start().unwrap();
    scheduler::run_ready().unwrap();
    assert_eq!(worker.state(), ThreadState::Dead(0));
    drop(worker);
    manager.reclaim_heap();
    assert_eq!(manager.heap_stats(), heap);
    assert_eq!(manager.stats(), frames);
}

fn entry_rollback_and_creator_exit(set_floating_enabled: fn(bool)) {
    let owner = Thread::new(
        |_| {
            *BOOT.lock() = Some(Fixture::new(73, 4));
            let wait = thread::prepare_wait().unwrap();
            *OWNER.lock() = Some(wait.waker());
            wait.wait().unwrap();
            19
        },
        0,
    )
    .unwrap();
    owner.start().unwrap();
    scheduler::run_ready().unwrap();
    assert_eq!(owner.state(), ThreadState::Blocked);
    let fixture = BOOT.lock().take().unwrap();
    fixture.thread.start().unwrap();
    assert_eq!(fixture.task.state(), TaskState::Active);
    set_floating_enabled(true);
    let result = scheduler::run_ready();
    set_floating_enabled(false);
    assert_eq!(
        result,
        Err(ThreadError::Execution(ExecutionError::ExtensionsEnabled))
    );
    assert_eq!(fixture.thread.state(), ThreadState::Ready);
    assert!(thread::current().is_none());
    assert!(OWNER.lock().take().unwrap().wake());
    scheduler::run_ready().unwrap();
    assert_eq!(owner.state(), ThreadState::Dead(19));
    fixture.assert_exit(73);
    assert_eq!(fixture.marker(), 73);
}

fn scenarios(_: usize) -> usize {
    distinct_spaces_and_start();
    cancelled_continuations();
    never_entered_and_faulted();
    fault_isolation();
    0
}

fn distinct_spaces_and_start() {
    let source = Task::new().unwrap();
    let first = Fixture::new_in(source.create_child().unwrap(), 42, 4);
    let second = Fixture::new(73, 4);
    assert_eq!(first.entry.pc, second.entry.pc);
    assert_eq!(first.entry.stack_pointer, second.entry.stack_pointer);
    let extra = first
        .task
        .create_thread(UserEntry {
            argument: 91,
            ..first.entry
        })
        .unwrap();
    let handle = source
        .insert(
            Object::Task(source.clone()),
            Rights::TRANSFER | Rights::WAIT,
        )
        .unwrap();
    let installed = first
        .task
        .install_handle(
            &source,
            TransferRequest {
                handle,
                rights: Rights::WAIT,
            },
        )
        .unwrap();
    assert!(
        first
            .task
            .get(installed, kernel::task::ObjectType::Task, Rights::WAIT)
            .is_ok()
    );
    let handle = source
        .insert(
            Object::Task(source.clone()),
            Rights::TRANSFER | Rights::WAIT,
        )
        .unwrap();
    first.thread.start().unwrap();
    assert!(matches!(
        first.task.install_handle(
            &source,
            TransferRequest {
                handle,
                rights: Rights::WAIT
            }
        ),
        Err(TaskError::InvalidState)
    ));
    assert!(
        source
            .get(handle, kernel::task::ObjectType::Task, Rights::WAIT)
            .is_ok()
    );
    second.thread.start().unwrap();
    wait_dead(&first.thread);
    wait_dead(&second.thread);
    second.assert_exit(73);
    assert_eq!(first.task.state(), TaskState::Active);
    extra.start().unwrap();
    wait_dead(&extra);
    assert_eq!(first.task.state(), TaskState::Dead(91));
    assert_eq!(first.marker(), 91);
    assert_eq!(second.marker(), 73);
    source.terminate(0);
}

fn cancelled_continuations() {
    let ready = Fixture::new(1, u64::MAX);
    ready.thread.start().unwrap();
    thread::yield_now().unwrap();
    ready.task.terminate(42);
    ready.task.terminate(99);
    assert_eq!(ready.task.state(), TaskState::Dying(42));
    wait_dead(&ready.thread);
    ready.assert_exit(42);
    assert_eq!(ready.marker(), 0);

    let target = Task::new().unwrap();
    let blocked = Fixture::new(2, 1);
    *WAIT_TARGET.lock() = Some(target.clone());
    blocked.thread.start().unwrap();
    thread::yield_now().unwrap();
    assert_eq!(blocked.thread.state(), ThreadState::Blocked);
    blocked.thread.terminate(19).unwrap();
    wait_dead(&blocked.thread);
    blocked.assert_exit(19);
    *WAIT_TARGET.lock() = None;
    assert_eq!(blocked.marker(), 0);
    assert_eq!(target.state(), TaskState::Created);
    target.terminate(0);
}

fn never_entered_and_faulted() {
    let invalidated = Fixture::new(1, 0);
    invalidated
        .task
        .unmap(VirtRange {
            start: invalidated.entry.pc,
            end: invalidated.entry.pc + PAGE_SIZE,
        })
        .unwrap();
    assert_eq!(
        invalidated.thread.start(),
        Err(ThreadError::Execution(ExecutionError::InvalidEntry))
    );
    assert_eq!(invalidated.task.state(), TaskState::Created);
    assert_eq!(invalidated.thread.state(), ThreadState::Created);
    invalidated.task.terminate(17);
    invalidated.assert_exit(17);
    for start in [false, true] {
        let fixture = Fixture::new(1, 0);
        if start {
            fixture.thread.start().unwrap();
        }
        fixture.task.terminate(17);
        fixture.assert_exit(17);
        assert_eq!(fixture.marker(), 0);
    }
    let fixture = Fixture::new(1, u64::MAX - 1);
    fixture.thread.start().unwrap();
    wait_dead(&fixture.thread);
    fixture.assert_exit(usize::MAX);
    assert_eq!(fixture.marker(), 0);
}

fn fault_isolation() {
    let faulted = Fixture::new(1, u64::MAX - 1);
    let sibling = Fixture::new_in(faulted.task.clone(), 73, 4);
    let child = faulted.task.create_child().unwrap();
    faulted.thread.start().unwrap();
    sibling.thread.start().unwrap();
    wait_dead(&faulted.thread);
    assert_eq!(faulted.thread.state(), ThreadState::Dead(usize::MAX));
    assert_eq!(faulted.marker(), 0);
    assert_eq!(faulted.task.state(), TaskState::Active);
    assert_eq!(sibling.thread.state(), ThreadState::Ready);
    assert_eq!(child.state(), TaskState::Created);
    assert!(
        sibling
            .task
            .query(sibling.entry.pc)
            .unwrap()
            .unwrap()
            .mapping
            .is_some()
    );
    wait_dead(&sibling.thread);
    sibling.assert_exit(73);
    assert_eq!(sibling.marker(), 73);
    assert_eq!(child.state(), TaskState::Dead(73));
}

fn wait_dead(thread: &Thread) {
    let deadline = hardware::timer().now_ns().unwrap() + 1_000_000_000;
    while !matches!(thread.state(), ThreadState::Dead(_)) {
        assert!(
            hardware::timer().now_ns().unwrap() < deadline,
            "user thread did not finish"
        );
        thread::yield_now().unwrap();
    }
}

impl Fixture {
    fn new(status: usize, rounds: u64) -> Self {
        Self::new_in(Task::new().unwrap(), status, rounds)
    }

    fn new_in(task: Task, status: usize, rounds: u64) -> Self {
        let manager = memory::manager();
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
        let entry = UserEntry {
            pc: text.start,
            stack_pointer: range.end,
            thread_pointer: range.start,
            argument: status,
        };
        let thread = task.create_thread(entry).unwrap();
        let fixture = Self {
            task,
            thread,
            stack,
            entry,
        };
        fixture.write_rounds(rounds);
        fixture
    }

    fn write_rounds(&self, rounds: u64) {
        let access = self
            .stack
            .access(
                self.stack.frames().next().unwrap(),
                ReferenceKind::Temporary,
                FrameAccess::Write,
            )
            .unwrap();
        access.write(0, &[rounds]).unwrap();
    }

    fn marker(&self) -> u64 {
        let access = self
            .stack
            .access(
                self.stack.frames().last().unwrap(),
                ReferenceKind::Temporary,
                FrameAccess::Read,
            )
            .unwrap();
        let mut word = [0];
        access.read(PAGE_SIZE / 8 - 2, &mut word).unwrap();
        word[0]
    }

    fn assert_exit(&self, status: usize) {
        assert_eq!(self.thread.state(), ThreadState::Dead(status));
        assert_eq!(self.task.state(), TaskState::Dead(status));
    }
}
