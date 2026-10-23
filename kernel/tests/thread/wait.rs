// SPDX-License-Identifier: MPL-2.0

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

use kernel::{
    hardware,
    thread::{self, Thread, ThreadState, WaitOutcome, Waker, scheduler},
};

static WAKERS: Mutex<[Option<Waker>; 4]> = Mutex::new([const { None }; 4]);
static COMPLETED: AtomicUsize = AtomicUsize::new(0);
static DEADLINE: AtomicU64 = AtomicU64::new(0);

pub fn run() {
    immediate();
    long_deadline();
    deadline_round_trips();
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
