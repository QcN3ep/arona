// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use alloc::{sync::Arc, vec::Vec};
use runtime::{Error, KernelError, ipc::WaitMode, sync::Semaphore, thread};

pub(super) fn run() {
    let semaphore = Semaphore::new(0);
    assert_eq!(
        semaphore.wait(WaitMode::Poll),
        Err(Error::Kernel(KernelError::WouldBlock))
    );
    for _ in 0..3 {
        semaphore.post().unwrap();
    }
    assert_eq!(
        semaphore.wait(WaitMode::Until(0)),
        Err(Error::Kernel(KernelError::InvalidArgument))
    );
    for _ in 0..3 {
        semaphore.wait(WaitMode::Poll).unwrap();
    }
    assert_eq!(
        semaphore.wait(WaitMode::Poll),
        Err(Error::Kernel(KernelError::WouldBlock))
    );
    assert_eq!(
        semaphore.wait(WaitMode::Until(thread::now_ns().unwrap() + 1_000_000)),
        Err(Error::Kernel(KernelError::TimedOut))
    );
    semaphore.post().unwrap();
    semaphore.wait(WaitMode::Poll).unwrap();

    let full = Semaphore::new(usize::MAX);
    assert_eq!(full.post(), Err(Error::Kernel(KernelError::InvalidState)));
    full.wait(WaitMode::Poll).unwrap();
    full.post().unwrap();

    multiple_waiters();
    deadline_race();
}

fn multiple_waiters() {
    let semaphore = Arc::new(Semaphore::new(0));
    let mut workers = Vec::new();
    for index in 0..4 {
        let waiting = semaphore.clone();
        let worker = thread::spawn(move || {
            waiting.wait(WaitMode::Forever).unwrap();
            index
        })
        .unwrap();
        super::blocked(&worker);
        workers.push(worker);
    }
    for _ in &workers {
        semaphore.post().unwrap();
    }
    assert_eq!(
        semaphore.wait(WaitMode::Poll),
        Err(Error::Kernel(KernelError::WouldBlock))
    );
    for (index, worker) in workers.into_iter().enumerate() {
        assert_eq!(worker.join().unwrap(), index);
    }
}

fn deadline_race() {
    let semaphore = Arc::new(Semaphore::new(0));
    let waiting = semaphore.clone();
    let worker =
        thread::spawn(move || waiting.wait(WaitMode::Until(thread::now_ns().unwrap() + 1_000_000)))
            .unwrap();
    thread::yield_now().unwrap();
    semaphore.post().unwrap();
    match worker.join().unwrap() {
        Ok(()) => assert_eq!(
            semaphore.wait(WaitMode::Poll),
            Err(Error::Kernel(KernelError::WouldBlock))
        ),
        Err(error) => {
            assert_eq!(error, Error::Kernel(KernelError::TimedOut));
            semaphore.wait(WaitMode::Poll).unwrap();
        }
    }
    assert_eq!(
        semaphore.wait(WaitMode::Poll),
        Err(Error::Kernel(KernelError::WouldBlock))
    );
}
