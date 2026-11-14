// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

use kernel::{
    hardware, memory,
    thread::{self, Thread, ThreadError, ThreadState, WaitOutcome, Waker, scheduler},
};

static WAKERS: Mutex<[Option<Waker>; 4]> = Mutex::new([const { None }; 4]);
static COMPLETED: AtomicUsize = AtomicUsize::new(0);
static DEADLINE: AtomicU64 = AtomicU64::new(0);
static TARGET: Mutex<Option<Thread>> = Mutex::new(None);

pub fn run() {
    immediate();
    long_deadline();
    deadline_round_trips();
    exit_immediate();
    exit_round_trips();
    exit_deadlines();
    exit_completion_order();
}

fn immediate() {
    let immediate = Thread::new(
        |_| {
            let wait = thread::prepare_wait().unwrap();
            let waker = wait.waker();
            assert!(waker.wake());
            assert_eq!(wait.wait_until(0), Ok(WaitOutcome::Notified));
            assert!(!waker.wake());
            let wait = thread::prepare_wait().unwrap();
            let waker = wait.waker();
            assert_eq!(wait.wait_until(0), Ok(WaitOutcome::TimedOut));
            assert!(!waker.wake());
            thread::sleep_until(0).unwrap();
            0
        },
        0,
    )
    .unwrap();
    immediate.start().unwrap();
    scheduler::run_ready().unwrap();
    assert_eq!(immediate.state(), ThreadState::Dead(0));
}

fn long_deadline() {
    let worker = Thread::new(
        |_| {
            let wait = thread::prepare_wait().unwrap();
            WAKERS.lock()[0] = Some(wait.waker());
            assert_eq!(wait.wait_until(u64::MAX), Ok(WaitOutcome::Notified));
            0
        },
        0,
    )
    .unwrap();
    worker.start().unwrap();
    scheduler::run_ready().unwrap();
    assert_eq!(worker.state(), ThreadState::Blocked);
    assert!(WAKERS.lock()[0].as_ref().unwrap().wake());
    scheduler::run_ready().unwrap();
    assert_eq!(worker.state(), ThreadState::Dead(0));
    WAKERS.lock()[0] = None;
}

fn deadline_round_trips() {
    let workers: [Thread; 4] = core::array::from_fn(|index| {
        Thread::new(
            |index| {
                let wait = thread::prepare_wait().unwrap();
                WAKERS.lock()[index] = Some(wait.waker());
                let deadline = DEADLINE.load(Ordering::Relaxed);
                let outcome = wait.wait_until(if index == 0 { u64::MAX } else { deadline });
                let expected = if index < 2 {
                    WaitOutcome::Notified
                } else {
                    WaitOutcome::TimedOut
                };
                assert_eq!(outcome, Ok(expected));
                assert!(!WAKERS.lock()[index].as_ref().unwrap().wake());
                if expected == WaitOutcome::TimedOut {
                    assert!(hardware::timer().now_ns().unwrap() >= deadline);
                }
                COMPLETED.fetch_add(1, Ordering::Relaxed);
                index
            },
            index,
        )
        .unwrap()
    });
    let deadline = hardware::timer().now_ns().unwrap() + 100_000_000;
    DEADLINE.store(deadline, Ordering::Relaxed);
    for worker in &workers {
        worker.start().unwrap();
    }
    scheduler::run_ready().unwrap();
    assert!(
        workers
            .iter()
            .all(|worker| worker.state() == ThreadState::Blocked)
    );
    {
        let wakers = WAKERS.lock();
        for index in [1, 0] {
            assert!(wakers[index].as_ref().unwrap().wake());
            assert!(!wakers[index].as_ref().unwrap().wake());
        }
    }
    scheduler::run_ready().unwrap();
    let limit = deadline + 100_000_000;
    while COMPLETED.load(Ordering::Relaxed) != workers.len() {
        assert!(
            hardware::timer().now_ns().unwrap() < limit,
            "timed wait stalled"
        );
        hardware::cpu().idle_until_interrupt();
        scheduler::run_ready().unwrap();
    }
    for (index, worker) in workers.iter().enumerate() {
        assert_eq!(worker.state(), ThreadState::Dead(index));
    }
    *WAKERS.lock() = [const { None }; 4];
}

fn exit_immediate() {
    let target = Thread::new(|_| 73, 0).unwrap();
    target.start().unwrap();
    scheduler::run_ready().unwrap();
    assert_eq!(target.wait(), Err(ThreadError::InvalidState));
    *TARGET.lock() = Some(target);
    let waiter = Thread::new(
        |_| {
            let current = thread::current().unwrap();
            assert_eq!(current.wait(), Err(ThreadError::InvalidState));
            assert_eq!(current.wait_until(0), Err(ThreadError::InvalidState));
            let target = TARGET.lock().as_ref().unwrap().clone();
            assert_eq!(target.wait(), Ok(73));
            assert_eq!(target.wait_until(0), Ok(Some(73)));
            let wait = thread::prepare_wait().unwrap();
            assert_eq!(target.wait(), Err(ThreadError::InvalidState));
            assert!(wait.waker().wake());
            wait.wait().unwrap();
            0
        },
        0,
    )
    .unwrap();
    waiter.start().unwrap();
    scheduler::run_ready().unwrap();
    assert_eq!(waiter.state(), ThreadState::Dead(0));
    *TARGET.lock() = None;
}

fn exit_round_trips() {
    let target = Thread::new(
        |_| {
            super::inspect_stack(0);
            let wait = thread::prepare_wait().unwrap();
            WAKERS.lock()[0] = Some(wait.waker());
            wait.wait().unwrap();
            73
        },
        0,
    )
    .unwrap();
    *TARGET.lock() = Some(target.clone());
    let waiters: [Thread; 3] = core::array::from_fn(|index| {
        Thread::new(
            |index| {
                let target = TARGET.lock().as_ref().unwrap().clone();
                if index == 0 {
                    assert_eq!(target.wait(), Ok(73));
                } else {
                    assert_eq!(target.wait_until(u64::MAX), Ok(Some(73)));
                }
                assert_eq!(target.state(), ThreadState::Dead(73));
                let guard = super::STACKS[0].load(Ordering::Relaxed);
                let space = memory::manager().kernel_address_space();
                for address in
                    (guard..guard + 5 * hardware::mmu::PAGE_SIZE).step_by(hardware::mmu::PAGE_SIZE)
                {
                    assert_eq!(space.query(address), Ok(None));
                }
                assert_eq!(target.wait_until(0), Ok(Some(73)));
                index
            },
            index,
        )
        .unwrap()
    });
    target.start().unwrap();
    for waiter in &waiters {
        waiter.start().unwrap();
    }
    let before = memory::manager().heap_stats();
    scheduler::run_ready().unwrap();
    assert_eq!(memory::manager().heap_stats(), before);
    assert!(
        waiters
            .iter()
            .all(|waiter| waiter.state() == ThreadState::Blocked)
    );
    assert!(WAKERS.lock()[0].take().unwrap().wake());
    scheduler::run_ready().unwrap();
    for (index, waiter) in waiters.iter().enumerate() {
        assert_eq!(waiter.state(), ThreadState::Dead(index));
    }
    *TARGET.lock() = None;
}

fn exit_deadlines() {
    COMPLETED.store(0, Ordering::Relaxed);
    let target = Thread::new(|_| 73, 0).unwrap();
    *TARGET.lock() = Some(target.clone());
    let waiters: [Thread; 3] = core::array::from_fn(|index| {
        Thread::new(
            |index| {
                let target = TARGET.lock().as_ref().unwrap().clone();
                assert_eq!(target.wait_until(0), Ok(None));
                let offset = [10_000_000, 0, 20_000_000][index];
                let deadline = DEADLINE.load(Ordering::Relaxed) + offset;
                assert_eq!(target.wait_until(deadline), Ok(None));
                assert!(hardware::timer().now_ns().unwrap() >= deadline);
                COMPLETED.fetch_add(1, Ordering::Relaxed);
                if index == 1 {
                    assert_eq!(target.wait_until(u64::MAX), Ok(Some(73)));
                }
                index
            },
            index,
        )
        .unwrap()
    });
    let deadline = hardware::timer().now_ns().unwrap() + 100_000_000;
    DEADLINE.store(deadline, Ordering::Relaxed);
    for waiter in &waiters {
        waiter.start().unwrap();
    }
    scheduler::run_ready().unwrap();
    while COMPLETED.load(Ordering::Relaxed) != waiters.len() {
        assert!(hardware::timer().now_ns().unwrap() < deadline + 100_000_000);
        hardware::cpu().idle_until_interrupt();
        scheduler::run_ready().unwrap();
    }
    assert_eq!(waiters[1].state(), ThreadState::Blocked);
    target.start().unwrap();
    scheduler::run_ready().unwrap();
    for (index, waiter) in waiters.iter().enumerate() {
        assert_eq!(waiter.state(), ThreadState::Dead(index));
    }
    *TARGET.lock() = None;
}

fn exit_completion_order() {
    for timer_first in [false, true] {
        let target = Thread::new(|_| 73, 0).unwrap();
        *TARGET.lock() = Some(target.clone());
        let waiter = Thread::new(
            |timer_first| {
                let target = TARGET.lock().as_ref().unwrap().clone();
                let outcome = target.wait_until(DEADLINE.load(Ordering::Relaxed));
                assert_eq!(target.state(), ThreadState::Dead(73));
                assert_eq!(outcome, Ok((timer_first == 0).then_some(73)));
                0
            },
            usize::from(timer_first),
        )
        .unwrap();
        let deadline = hardware::timer().now_ns().unwrap() + 10_000_000;
        DEADLINE.store(deadline, Ordering::Relaxed);
        waiter.start().unwrap();
        scheduler::run_ready().unwrap();
        assert_eq!(waiter.state(), ThreadState::Blocked);
        target.start().unwrap();
        if timer_first {
            while waiter.state() == ThreadState::Blocked {
                assert!(hardware::timer().now_ns().unwrap() < deadline + 100_000_000);
                hardware::cpu().idle_until_interrupt();
            }
        } else {
            // Keep interrupts disabled; commit the exit first, then handle the expired timer.
            while hardware::timer().now_ns().unwrap() < deadline {
                core::hint::spin_loop();
            }
        }
        scheduler::run_ready().unwrap();
        assert_eq!(waiter.state(), ThreadState::Dead(0));
        *TARGET.lock() = None;
    }
}
