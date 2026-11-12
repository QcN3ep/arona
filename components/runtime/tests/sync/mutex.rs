// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use alloc::{sync::Arc, vec::Vec};
use core::cell::Cell;
use runtime::{Error, KernelError, ipc::WaitMode, sync::Mutex, thread};

pub(super) fn run() {
    let mutex = Arc::new(Mutex::new(Cell::new(0)));
    let held = mutex.lock(WaitMode::Forever).unwrap();
    assert!(matches!(
        mutex.lock(WaitMode::Poll),
        Err(Error::Kernel(KernelError::WouldBlock))
    ));
    let mut workers = Vec::new();
    for _ in 0..4 {
        let shared = mutex.clone();
        let worker = thread::spawn(move || {
            for _ in 0..16 {
                let guard = shared.lock(WaitMode::Forever).unwrap();
                let previous = guard.get();
                thread::yield_now().unwrap();
                guard.set(previous + 1);
            }
        })
        .unwrap();
        super::blocked(&worker);
        workers.push(worker);
    }
    held.set(10);
    drop(held);
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(mutex.lock(WaitMode::Poll).unwrap().get(), 74);

    let held = mutex.lock(WaitMode::Forever).unwrap();
    let waiting = mutex.clone();
    let worker = thread::spawn(move || {
        assert!(matches!(
            waiting.lock(WaitMode::Until(thread::now_ns().unwrap() + 1_000_000)),
            Err(Error::Kernel(KernelError::TimedOut))
        ));
    })
    .unwrap();
    worker.join().unwrap();
    assert_eq!(held.get(), 74);
    drop(held);
    assert_eq!(mutex.lock(WaitMode::Poll).unwrap().get(), 74);
}
