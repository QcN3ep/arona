// SPDX-License-Identifier: MPL-2.0

use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

use kernel::{
    hardware::{
        self,
        cpu::InterruptState,
        execution::ExecutionError,
        mmu::{PAGE_SIZE, PagePermissions},
    },
    memory::{self, MemoryError, VirtRange},
    thread::{self, Thread, ThreadError, ThreadState, WaitOutcome, Waker, scheduler},
    trap::{Trap, TrapEvent, TrapOrigin, TrapOutcome},
};

mod user;
mod wait;

pub use user::{handle as handle_user, run as run_user};

static EXECUTION_ORDER: AtomicUsize = AtomicUsize::new(0);
static EXECUTION_TARGET: Mutex<Option<Thread>> = Mutex::new(None);
static ORDER: AtomicUsize = AtomicUsize::new(0);
static WAKER: Mutex<Option<Waker>> = Mutex::new(None);
static STACKS: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];
static INTERRUPTS: AtomicUsize = AtomicUsize::new(0);

pub fn run<P: crate::Platform>() {
    scheduler::initialize().unwrap();
    assert_eq!(
        scheduler::initialize(),
        Err(ThreadError::AlreadyInitialized)
    );
    execution_environment::<P>();
    let manager = memory::manager();
    manager.reclaim_heap();
    let before = manager.heap_stats();
    let frames = manager.stats().kernel.free_frames;
    let first = Thread::new(waiter, 73).unwrap();
    let second = Thread::new(notifier, 42).unwrap();
    assert_eq!(first.state(), ThreadState::Created);
    assert_eq!(second.state(), ThreadState::Created);
    first.start().unwrap();
    second.start().unwrap();
    assert_eq!(first.start(), Err(ThreadError::InvalidState));
    assert_eq!(first.state(), ThreadState::Ready);
    assert!(thread::current().is_none());
    assert_eq!(thread::yield_now(), Err(ThreadError::InvalidState));
    scheduler::run_ready().unwrap();
    assert_eq!(ORDER.load(Ordering::Relaxed), 6);
    assert_eq!(first.state(), ThreadState::Dead(73));
    assert_eq!(second.state(), ThreadState::Dead(42));
    assert_eq!(first.start(), Err(ThreadError::InvalidState));
    assert!(!WAKER.lock().as_ref().unwrap().wake());
    assert!(thread::current().is_none());
    for stack in &STACKS {
        let guard = stack.load(Ordering::Relaxed);
        for address in (guard..guard + 5 * PAGE_SIZE).step_by(PAGE_SIZE) {
            assert_eq!(manager.kernel_address_space().query(address), Ok(None));
        }
    }
    assert_eq!(manager.heap_stats().allocations, before.allocations + 2);
    *WAKER.lock() = None;
    drop(first);
    drop(second);
    Thread::new(|_| 0, 0).unwrap().start().unwrap();
    scheduler::run_ready().unwrap();
    let parked = Thread::new(
        |_| {
            let wait = thread::prepare_wait().unwrap();
            *WAKER.lock() = Some(wait.waker());
            wait.wait().unwrap();
            91
        },
        0,
    )
    .unwrap();
    parked.start().unwrap();
    scheduler::run_ready().unwrap();
    assert_eq!(parked.state(), ThreadState::Blocked);
    assert!(WAKER.lock().as_ref().unwrap().wake());
    assert!(!WAKER.lock().as_ref().unwrap().wake());
    assert_eq!(parked.state(), ThreadState::Ready);
    scheduler::run_ready().unwrap();
    assert_eq!(parked.state(), ThreadState::Dead(91));
    assert!(!WAKER.lock().as_ref().unwrap().wake());
    *WAKER.lock() = None;
    drop(parked);
    drop(Thread::new(|_| panic!("unstarted thread executed"), 0).unwrap());
    interrupt_round_trips();
    deferred_preemption();
    wait::run();
    manager.reclaim_heap();
    assert_eq!(manager.heap_stats(), before);
    assert_eq!(manager.stats().kernel.free_frames, frames);
}

fn interrupt_round_trips() {
    for state in [InterruptState::Disabled, InterruptState::Enabled] {
        let count = INTERRUPTS.load(Ordering::Relaxed);
        let waiter = Thread::new(
            |_| {
                let wait = thread::prepare_wait().unwrap();
                let timer = hardware::timer();
                assert_eq!(
                    wait.wait_until(timer.now_ns().unwrap() + 1_000_000),
                    Ok(WaitOutcome::TimedOut)
                );
                73
            },
            0,
        )
        .unwrap();
        let ready = Thread::new(
            |count| {
                let limit = hardware::timer().now_ns().unwrap() + 100_000_000;
                while INTERRUPTS.load(Ordering::Relaxed) == count {
                    assert_eq!(hardware::cpu().interrupt_state(), InterruptState::Disabled);
                    assert!(
                        hardware::timer().now_ns().unwrap() < limit,
                        "timer starved by ready thread"
                    );
                    thread::yield_now().unwrap();
                }
                42
            },
            count,
        )
        .unwrap();
        waiter.start().unwrap();
        ready.start().unwrap();
        hardware::cpu().restore_interrupts(state);
        let result = scheduler::run_ready();
        let restored = hardware::cpu().disable_interrupts();
        assert_eq!(result, Ok(()));
        assert_eq!(restored, state);
        assert_eq!(INTERRUPTS.load(Ordering::Relaxed), count + 1);
        assert_eq!(waiter.state(), ThreadState::Dead(73));
        assert_eq!(ready.state(), ThreadState::Dead(42));
        *WAKER.lock() = None;
    }
}

pub fn handle(trap: Trap) -> TrapOutcome {
    assert_eq!(trap.origin, TrapOrigin::Kernel);
    assert_eq!(trap.event, TrapEvent::TimerInterrupt);
    assert_eq!(hardware::cpu().interrupt_state(), InterruptState::Disabled);
    if let Some(current) = thread::current() {
        assert!(current.task().is_none());
        assert_eq!(current.state(), ThreadState::Running);
    }
    let outcome = kernel::trap::dispatch(trap);
    assert_eq!(outcome, TrapOutcome::Resume);
    INTERRUPTS.fetch_add(1, Ordering::Relaxed);
    outcome
}

fn deferred_preemption() {
    let worker = Thread::new(
        |_| {
            let current = thread::current().unwrap();
            let count = INTERRUPTS.load(Ordering::Relaxed);
            scheduler::request_preemption().unwrap();
            scheduler::set_preemption_deadline(Some(
                hardware::timer().now_ns().unwrap() + 1_000_000,
            ))
            .unwrap();
            while INTERRUPTS.load(Ordering::Relaxed) == count {
                hardware::cpu().idle_until_interrupt();
            }
            assert_eq!(current.state(), ThreadState::Running);
            assert_eq!(ORDER.load(Ordering::Relaxed), 6);
            thread::yield_now().unwrap();
            assert_eq!(ORDER.load(Ordering::Relaxed), 7);
            73
        },
        0,
    )
    .unwrap();
    let observer = Thread::new(
        |_| {
            step(6);
            42
        },
        0,
    )
    .unwrap();
    worker.start().unwrap();
    observer.start().unwrap();
    scheduler::run_ready().unwrap();
    assert_eq!(worker.state(), ThreadState::Dead(73));
    assert_eq!(observer.state(), ThreadState::Dead(42));
}

fn waiter(argument: usize) -> usize {
    assert_eq!(argument, 73);
    assert_eq!(thread::current().unwrap().state(), ThreadState::Running);
    assert_eq!(scheduler::run_ready(), Err(ThreadError::InvalidState));
    inspect_stack(0);
    step(0);
    let early = thread::prepare_wait().unwrap();
    let stale = early.waker();
    *WAKER.lock() = Some(stale.clone());
    thread::yield_now().unwrap();
    early.wait().unwrap();
    assert!(!stale.wake());
    let cancelled = thread::prepare_wait().unwrap();
    let cancelled_waker = cancelled.waker();
    drop(cancelled);
    assert!(!cancelled_waker.wake());
    let wait = thread::prepare_wait().unwrap();
    assert!(!stale.wake());
    assert!(!cancelled_waker.wake());
    let stale = wait.waker();
    *WAKER.lock() = Some(stale.clone());
    wait.wait().unwrap();
    step(3);
    let wait = thread::prepare_wait().unwrap();
    assert!(!stale.wake());
    *WAKER.lock() = Some(wait.waker());
    wait.wait().unwrap();
    step(5);
    argument
}

fn notifier(argument: usize) -> usize {
    assert_eq!(argument, 42);
    inspect_stack(1);
    for expected in [1, 2, 4] {
        step(expected);
        {
            let waker = WAKER.lock();
            let waker = waker.as_ref().unwrap();
            assert!(waker.wake());
            assert!(!waker.wake());
        }
        if expected != 4 {
            thread::yield_now().unwrap();
        }
    }
    argument
}

fn step(expected: usize) {
    assert_eq!(ORDER.fetch_add(1, Ordering::Relaxed), expected);
}

fn inspect_stack(index: usize) {
    let local = 0;
    let address = &local as *const _ as usize;
    let space = memory::manager().kernel_address_space();
    let mut guard = address & !(PAGE_SIZE - 1);
    while space.translate(guard).unwrap().is_some() {
        guard -= PAGE_SIZE;
        assert!(address - guard <= 5 * PAGE_SIZE);
    }
    let guard_range = VirtRange {
        start: guard,
        end: guard + PAGE_SIZE,
    };
    assert!(space.query(guard).unwrap().is_some());
    assert_eq!(space.unreserve(guard_range), Err(MemoryError::InUse));
    let stack = VirtRange {
        start: guard_range.end,
        end: guard_range.end + 4 * PAGE_SIZE,
    };
    assert_eq!(space.unmap(stack), Err(MemoryError::InUse));
    assert_eq!(
        space.protect(stack, PagePermissions::ReadOnly),
        Err(MemoryError::InUse)
    );
    STACKS[index].store(guard, Ordering::Relaxed);
}

fn execution_environment<P: crate::Platform>() {
    let thread = Thread::new(execution_worker::<P>, 0).unwrap();
    let next = Thread::new(
        |_: usize| {
            assert_eq!(EXECUTION_ORDER.fetch_add(1, Ordering::Relaxed), 1);
            42
        },
        0,
    )
    .unwrap();
    *EXECUTION_TARGET.lock() = Some(next.clone());
    thread.start().unwrap();
    next.start().unwrap();
    P::set_floating_enabled(true);
    let result = scheduler::run_ready();
    P::set_floating_enabled(false);
    assert_eq!(
        result,
        Err(ThreadError::Execution(ExecutionError::ExtensionsEnabled))
    );
    assert_eq!(thread.state(), ThreadState::Ready);
    assert!(thread::current().is_none());
    scheduler::run_ready().unwrap();
    assert_eq!(thread.state(), ThreadState::Dead(19));
    assert_eq!(next.state(), ThreadState::Dead(42));
    *EXECUTION_TARGET.lock() = None;
    drop(thread);
    drop(next);
}

fn execution_worker<P: crate::Platform>(_: usize) -> usize {
    assert_eq!(EXECUTION_ORDER.fetch_add(1, Ordering::Relaxed), 0);
    P::set_floating_enabled(true);
    let result = thread::yield_now();
    P::set_floating_enabled(false);
    assert_eq!(
        result,
        Err(ThreadError::Execution(ExecutionError::ExtensionsEnabled))
    );
    assert_eq!(thread::current().unwrap().state(), ThreadState::Running);
    let wait = thread::prepare_wait().unwrap();
    let waker = wait.waker();
    P::set_floating_enabled(true);
    let result = wait.wait_until(u64::MAX);
    P::set_floating_enabled(false);
    assert_eq!(
        result,
        Err(ThreadError::Execution(ExecutionError::ExtensionsEnabled))
    );
    assert!(!waker.wake());
    let target = EXECUTION_TARGET.lock().as_ref().unwrap().clone();
    P::set_floating_enabled(true);
    let result = target.wait_until(u64::MAX);
    P::set_floating_enabled(false);
    assert_eq!(
        result,
        Err(ThreadError::Execution(ExecutionError::ExtensionsEnabled))
    );
    drop(thread::prepare_wait().unwrap());
    thread::yield_now().unwrap();
    19
}
