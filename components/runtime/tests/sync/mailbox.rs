// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use alloc::{sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicUsize, Ordering};
use runtime::{Error, KernelError, ipc::WaitMode, sync::Mailbox, thread};

pub(super) fn run() {
    assert!(matches!(
        Mailbox::<u8>::new(0),
        Err(Error::Kernel(KernelError::InvalidArgument))
    ));
    let mailbox = Mailbox::new(2).unwrap();
    mailbox.send(10, WaitMode::Poll).unwrap();
    mailbox.send(20, WaitMode::Poll).unwrap();
    assert_eq!(
        mailbox.send(30, WaitMode::Poll),
        Err((Error::Kernel(KernelError::WouldBlock), 30))
    );
    assert_eq!(
        mailbox.send(30, WaitMode::Until(thread::now_ns().unwrap() + 1_000_000)),
        Err((Error::Kernel(KernelError::TimedOut), 30))
    );
    assert_eq!(mailbox.receive(WaitMode::Poll).unwrap(), 10);
    assert_eq!(mailbox.receive(WaitMode::Poll).unwrap(), 20);
    assert_eq!(
        mailbox.receive(WaitMode::Poll),
        Err(Error::Kernel(KernelError::WouldBlock))
    );

    blocked_senders();
    blocked_receivers();
    ownership();
    allocation_failure();
}

fn blocked_senders() {
    let mailbox = Arc::new(Mailbox::new(1).unwrap());
    mailbox.send(0, WaitMode::Poll).unwrap();
    let mut workers = Vec::new();
    for value in 1..=3 {
        let sending = mailbox.clone();
        let worker =
            thread::spawn(move || sending.send(value, WaitMode::Forever).unwrap()).unwrap();
        super::blocked(&worker);
        workers.push(worker);
    }
    for value in 0..=3 {
        assert_eq!(mailbox.receive(WaitMode::Forever).unwrap(), value);
    }
    for worker in workers {
        worker.join().unwrap();
    }
}

fn blocked_receivers() {
    let mailbox = Arc::new(Mailbox::new(3).unwrap());
    let mut workers = Vec::new();
    for _ in 0..3 {
        let receiving = mailbox.clone();
        let worker = thread::spawn(move || receiving.receive(WaitMode::Forever).unwrap()).unwrap();
        super::blocked(&worker);
        workers.push(worker);
    }
    for value in 0..3 {
        mailbox.send(value, WaitMode::Poll).unwrap();
    }
    let mut received: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    received.sort_unstable();
    assert_eq!(received, [0, 1, 2]);
    assert_eq!(
        mailbox.receive(WaitMode::Poll),
        Err(Error::Kernel(KernelError::WouldBlock))
    );
}

#[derive(Debug)]
struct Owned(Arc<AtomicUsize>);

impl Drop for Owned {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn ownership() {
    let drops = Arc::new(AtomicUsize::new(0));
    let mailbox = Mailbox::new(1).unwrap();
    mailbox.send(Owned(drops.clone()), WaitMode::Poll).unwrap();
    let (error, returned) = mailbox
        .send(Owned(drops.clone()), WaitMode::Poll)
        .unwrap_err();
    assert_eq!(error, Error::Kernel(KernelError::WouldBlock));
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(returned);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    drop(mailbox);
    assert_eq!(drops.load(Ordering::SeqCst), 2);
}

fn allocation_failure() {
    const CHUNK: usize = 1024 * 1024;
    let existing = Mailbox::new(1).unwrap();
    existing.send(42, WaitMode::Poll).unwrap();
    let mut blocks = Vec::new();
    blocks.try_reserve_exact(128).unwrap();
    loop {
        let mut block = Vec::<u8>::new();
        if block.try_reserve_exact(CHUNK).is_err() {
            break;
        }
        blocks.push(block);
        assert!(blocks.len() < 128);
    }
    assert!(matches!(
        Mailbox::<u8>::new(CHUNK),
        Err(Error::Kernel(KernelError::NoMemory))
    ));
    assert_eq!(existing.receive(WaitMode::Poll).unwrap(), 42);
    existing.send(43, WaitMode::Poll).unwrap();
    assert_eq!(existing.receive(WaitMode::Poll).unwrap(), 43);
    drop(blocks);
    let recovered = Mailbox::new(CHUNK).unwrap();
    recovered.send(7u8, WaitMode::Poll).unwrap();
    assert_eq!(recovered.receive(WaitMode::Poll).unwrap(), 7);
}
