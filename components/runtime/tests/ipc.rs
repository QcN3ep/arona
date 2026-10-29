// SPDX-License-Identifier: MPL-2.0

use alloc::{sync::Arc, vec::Vec};
use runtime::{
    Error, KernelError, Rights,
    ipc::{self, Message, WaitItem, WaitMode, WaitResult},
    process,
    thread::{self, ThreadState},
};

pub fn run() {
    let rights = Rights::READ_WRITE | Rights::WAIT;
    let mut senders = Vec::new();
    let mut receivers = Vec::new();
    for _ in 0..33 {
        let (sender, receiver) = process::task().channel(rights, rights).unwrap();
        senders.push(sender);
        receivers.push(receiver);
    }
    let receivers = Arc::new(receivers);
    let items: Vec<_> = receivers
        .iter()
        .map(|channel| WaitItem::new(channel, ipc::READABLE | ipc::PEER_CLOSED))
        .collect();
    let last = items.len() - 1;
    assert_eq!(
        ipc::wait_many(&items, WaitMode::Poll),
        Err(Error::Kernel(KernelError::WouldBlock))
    );
    senders[last]
        .send(&mut Message::new(73, &[]).unwrap(), WaitMode::Poll)
        .unwrap();
    assert_eq!(
        ipc::wait_many(&items, WaitMode::Until(thread::now_ns().unwrap())),
        Ok(WaitResult {
            index: last,
            ready: ipc::READABLE,
        })
    );
    assert_eq!(receivers[last].receive(WaitMode::Poll).unwrap().tag, 73);
    let waiting = Arc::clone(&receivers);
    let worker = thread::Builder::new()
        .stack_size(64 * 1024)
        .spawn(move || {
            let items: Vec<_> = waiting
                .iter()
                .map(|channel| WaitItem::new(channel, ipc::READABLE | ipc::PEER_CLOSED))
                .collect();
            ipc::wait_many(&items, WaitMode::Forever)
        })
        .unwrap();
    super::wait_for(|| worker.thread().state().unwrap().0 == ThreadState::Blocked);
    senders[last]
        .send(&mut Message::new(74, &[]).unwrap(), WaitMode::Poll)
        .unwrap();
    assert_eq!(
        worker.join().unwrap(),
        Ok(WaitResult {
            index: last,
            ready: ipc::READABLE,
        })
    );
    assert_eq!(receivers[last].receive(WaitMode::Poll).unwrap().tag, 74);
    drop(senders.pop());
    assert_eq!(
        ipc::wait_many(&items, WaitMode::Poll),
        Ok(WaitResult {
            index: last,
            ready: ipc::PEER_CLOSED,
        })
    );
}
