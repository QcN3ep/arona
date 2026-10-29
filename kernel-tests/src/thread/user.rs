// SPDX-License-Identifier: MPL-2.0

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use spin::{Mutex, Once};

use kernel::{
    hardware::{
        self,
        execution::{ExecutionError, UserEntry},
        mmu::{PAGE_SIZE, PagePermissions},
    },
    ipc::{Channel, ChannelError, Event, Message, WaitItem, WaitManyError},
    memory::{
        self, FrameAccess, FrameAllocation, FrameOwner, FramePurpose, MappingOptions,
        ReferenceKind, VirtRange,
    },
    task::{Handle, Object, Rights, Task, TaskError, TaskState, TransferRequest},
    thread::{self, Thread, ThreadError, ThreadState, Waker, scheduler},
    trap::{Trap, TrapEvent, TrapOutcome},
};

static CODE: Once<[u64; PAGE_SIZE / 8]> = Once::new();
static BOOT: Mutex<Option<Fixture>> = Mutex::new(None);
static OWNER: Mutex<Option<Waker>> = Mutex::new(None);
static WAIT_EVENT: Mutex<Option<(Task, Handle)>> = Mutex::new(None);
static WAIT_CALL: Mutex<Option<(Task, Handle)>> = Mutex::new(None);
static WAIT_TARGET: Mutex<Option<Task>> = Mutex::new(None);
static TIMERS: AtomicUsize = AtomicUsize::new(0);
static SLEEP_START: AtomicU64 = AtomicU64::new(0);
static REQUEST_ON_TIMER: AtomicBool = AtomicBool::new(false);
static RELEASE_ON_TIMER: AtomicBool = AtomicBool::new(false);

const BUSY: u64 = u64::MAX - 2;

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
    if trap.event == TrapEvent::TimerInterrupt {
        TIMERS.fetch_add(1, Ordering::Relaxed);
        if REQUEST_ON_TIMER.swap(false, Ordering::Relaxed) {
            scheduler::request_preemption().unwrap();
        }
        if RELEASE_ON_TIMER.swap(false, Ordering::Relaxed) {
            let fixture = BOOT.lock();
            let fixture = fixture.as_ref().unwrap();
            assert_eq!(fixture.entered(), fixture.entry.argument as u64);
            fixture.write_tls(1, &[1]);
        }
    }
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
    let call = WAIT_CALL.lock().clone();
    if let Some((task, handle)) = call {
        assert_eq!(
            task.call(handle, Message::new(42, &[]).unwrap(), &[]),
            Err(TaskError::Channel(ChannelError::Thread(
                ThreadError::Terminated
            )))
        );
        return Action::Dispatch;
    }
    let event = WAIT_EVENT.lock().clone();
    if let Some((task, handle)) = event {
        assert_eq!(
            task.wait_many(&[WaitItem {
                handle,
                condition_mask: 1
            }]),
            Err(TaskError::Wait(WaitManyError::Thread(
                ThreadError::Terminated
            )))
        );
        return Action::Dispatch;
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
    preemption();
    preemption_deadlines();
    let worker = Thread::new(scenarios, 0).unwrap();
    worker.start().unwrap();
    scheduler::run_ready().unwrap();
    assert_eq!(worker.state(), ThreadState::Dead(0));
    drop(worker);
    manager.reclaim_heap();
    assert_eq!(manager.heap_stats(), heap);
    assert_eq!(manager.stats(), frames);
}

fn preemption() {
    let (fixture, creator) = preemption_fixture(73);
    fixture.thread.start().unwrap();
    *BOOT.lock() = Some(fixture);
    RELEASE_ON_TIMER.store(true, Ordering::Relaxed);
    let count = TIMERS.load(Ordering::Relaxed);
    scheduler::set_preemption_deadline(Some(hardware::timer().now_ns().unwrap() + 5_000_000))
        .unwrap();
    scheduler::run_ready().unwrap();
    assert_eq!(TIMERS.load(Ordering::Relaxed), count + 1);
    let fixture = BOOT.lock().take().unwrap();
    fixture.assert_exit(73);
    assert_eq!(fixture.marker(), 73);
    drop(fixture);
    assert!(creator.wake());
    scheduler::run_ready().unwrap();
    drop(creator);

    for terminate in [false, true] {
        let (fixture, creator) = preemption_fixture(73);
        let (other, other_creator) = preemption_fixture(42);
        let observer = Thread::new(preemption_observer, usize::from(terminate)).unwrap();
        fixture.thread.start().unwrap();
        other.thread.start().unwrap();
        observer.start().unwrap();
        *BOOT.lock() = Some(fixture);
        scheduler::set_preemption_deadline(Some(hardware::timer().now_ns().unwrap() + 5_000_000))
            .unwrap();
        scheduler::run_ready().unwrap();
        let fixture = BOOT.lock().take().unwrap();
        fixture.assert_exit(if terminate { 19 } else { 73 });
        assert_eq!(fixture.marker(), if terminate { 0 } else { 73 });
        other.assert_exit(42);
        assert_eq!(other.marker(), 42);
        assert_eq!(observer.state(), ThreadState::Dead(0));
        assert!(creator.wake());
        assert!(other_creator.wake());
        scheduler::run_ready().unwrap();
    }
}

fn preemption_fixture(status: usize) -> (Fixture, Waker) {
    let creator = Thread::new(
        |status| {
            *BOOT.lock() = Some(Fixture::new(status, if status == 73 { BUSY } else { 2 }));
            let wait = thread::prepare_wait().unwrap();
            *OWNER.lock() = Some(wait.waker());
            wait.wait().unwrap();
            0
        },
        status,
    )
    .unwrap();
    creator.start().unwrap();
    scheduler::run_ready().unwrap();
    assert_eq!(creator.state(), ThreadState::Blocked);
    (BOOT.lock().take().unwrap(), OWNER.lock().take().unwrap())
}

fn preemption_observer(terminate: usize) -> usize {
    let fixture = BOOT.lock();
    let fixture = fixture.as_ref().unwrap();
    assert_eq!(fixture.thread.state(), ThreadState::Ready);
    assert_eq!(fixture.entered(), 73);
    assert_eq!(fixture.marker(), 0);
    fixture.write_tls(1, &[1]);
    if terminate != 0 {
        fixture.thread.terminate(19).unwrap();
    }
    0
}

fn preemption_deadlines() {
    for (wait_first, request) in [(true, false), (false, false), (true, true)] {
        let (fixture, creator) = preemption_fixture(73);
        let observer = Thread::new(preemption_observer, 0).unwrap();
        let waiter = Thread::new(
            |wait_first| {
                let now = hardware::timer().now_ns().unwrap();
                SLEEP_START.store(now, Ordering::Relaxed);
                let delay = if wait_first != 0 {
                    50_000_000
                } else {
                    100_000_000
                };
                thread::sleep_until(now + delay).unwrap();
                42
            },
            usize::from(wait_first),
        )
        .unwrap();
        waiter.start().unwrap();
        scheduler::run_ready().unwrap();
        assert_eq!(waiter.state(), ThreadState::Blocked);
        let now = SLEEP_START.load(Ordering::Relaxed);
        let early = now + 50_000_000;
        let late = now + 100_000_000;
        fixture.thread.start().unwrap();
        observer.start().unwrap();
        *BOOT.lock() = Some(fixture);
        let count = TIMERS.load(Ordering::Relaxed);
        if request {
            REQUEST_ON_TIMER.store(true, Ordering::Relaxed);
        } else {
            scheduler::set_preemption_deadline(Some(now)).unwrap();
            scheduler::set_preemption_deadline(None).unwrap();
            scheduler::set_preemption_deadline(Some(late)).unwrap();
            scheduler::set_preemption_deadline(Some(if wait_first { late } else { early }))
                .unwrap();
        }
        scheduler::run_ready().unwrap();
        assert_eq!(
            TIMERS.load(Ordering::Relaxed),
            count + if wait_first && !request { 2 } else { 1 }
        );
        let fixture = BOOT.lock().take().unwrap();
        fixture.assert_exit(73);
        assert_eq!(fixture.marker(), 73);
        assert_eq!(observer.state(), ThreadState::Dead(0));
        while waiter.state() == ThreadState::Blocked {
            hardware::cpu().idle_until_interrupt();
            scheduler::run_ready().unwrap();
        }
        assert_eq!(waiter.state(), ThreadState::Dead(42));
        assert!(creator.wake());
        scheduler::run_ready().unwrap();
    }
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
    cancelled_ipc_wait();
    cancelled_ipc_call();
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

fn cancelled_ipc_wait() {
    let target = Task::new().unwrap();
    let event = Event::new().unwrap();
    let handle = target
        .insert(Object::Event(event.clone()), Rights::WAIT)
        .unwrap();
    let blocked = Fixture::new(2, 1);
    *WAIT_EVENT.lock() = Some((target.clone(), handle));
    blocked.thread.start().unwrap();
    thread::yield_now().unwrap();
    assert_eq!(blocked.thread.state(), ThreadState::Blocked);
    blocked.thread.terminate(19).unwrap();
    event.signal(1).unwrap();
    assert_eq!(
        target
            .try_wait_many(&[WaitItem {
                handle,
                condition_mask: 1
            }])
            .unwrap()
            .ready,
        1
    );
    wait_dead(&blocked.thread);
    blocked.assert_exit(19);
    assert_eq!(blocked.marker(), 0);
    WAIT_EVENT.lock().take();
    target.terminate(0);
}

fn cancelled_ipc_call() {
    for completed in [false, true] {
        cancel_call(completed);
    }
}

fn cancel_call(completed: bool) {
    let task = Task::new().unwrap();
    let (sender, receiver) = Channel::pair().unwrap();
    let sender = task
        .insert(Object::Channel(sender), Rights::READ | Rights::WRITE)
        .unwrap();
    let receiver = task
        .insert(Object::Channel(receiver), Rights::READ)
        .unwrap();
    let blocked = Fixture::new(2, 1);
    *WAIT_CALL.lock() = Some((task.clone(), sender));
    blocked.thread.start().unwrap();
    thread::yield_now().unwrap();
    assert_eq!(blocked.thread.state(), ThreadState::Blocked);
    let request = task.receive(receiver, 8).unwrap();
    assert!(request.has_reply());
    let allocation = memory::manager()
        .allocate(FrameOwner::User(1), FramePurpose::Data, 1)
        .unwrap();
    let allocation = memory::manager().allocate_arc(allocation).unwrap();
    if completed {
        let frame = task
            .insert(
                Object::FrameAllocation(allocation.clone()),
                Rights::TRANSFER | Rights::READ,
            )
            .unwrap();
        task.reply(
            request.handles()[0],
            Message::new(73, &[]).unwrap(),
            &[TransferRequest {
                handle: frame,
                rights: Rights::READ,
            }],
        )
        .unwrap();
        assert_eq!(blocked.thread.state(), ThreadState::Ready);
    }
    blocked.thread.terminate(19).unwrap();
    wait_dead(&blocked.thread);
    blocked.assert_exit(19);
    assert_eq!(memory::HeapArc::strong_count(&allocation), 1);
    if !completed {
        assert_eq!(
            task.reply(request.handles()[0], Message::new(73, &[]).unwrap(), &[]),
            Err(TaskError::Channel(ChannelError::InvalidState))
        );
    }
    WAIT_CALL.lock().take();
    task.terminate(0);
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
        fixture.write_tls(0, &[rounds, 0, 0]);
        fixture
    }

    fn write_tls(&self, word_offset: usize, words: &[u64]) {
        let access = self
            .stack
            .access(
                self.stack.frames().next().unwrap(),
                ReferenceKind::Temporary,
                FrameAccess::Write,
            )
            .unwrap();
        access.write(word_offset, words).unwrap();
    }

    fn entered(&self) -> u64 {
        let access = self
            .stack
            .access(
                self.stack.frames().next().unwrap(),
                ReferenceKind::Temporary,
                FrameAccess::Read,
            )
            .unwrap();
        let mut word = [0];
        access.read(2, &mut word).unwrap();
        word[0]
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
