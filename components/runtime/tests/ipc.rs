// SPDX-License-Identifier: MPL-2.0

use alloc::{sync::Arc, vec::Vec};
use runtime::{
    Error, KernelError, Rights,
    ipc::{self, Message, Transfer, WaitItem, WaitMode, WaitResult},
    process,
    thread::{self, ThreadState},
};

pub fn run() {
    calls();
    cancelled_calls();
    call_backpressure();
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

fn calls() {
    let rights = Rights::READ_WRITE | Rights::WAIT;
    let (client, server) = process::task().channel(rights, rights).unwrap();
    let event = process::task()
        .event(Rights::TRANSFER | Rights::WAIT)
        .unwrap();
    let mut request = Message::new(1, &[42]).unwrap();
    request
        .push(Transfer {
            handle: event.into(),
            rights: Rights::TRANSFER | Rights::WAIT,
        })
        .unwrap();
    let first = client.start_call(&mut request).unwrap();
    assert!(request.transfers().is_empty());
    let second = client
        .start_call(&mut Message::new(2, &[]).unwrap())
        .unwrap();
    assert_eq!(
        first.try_receive().unwrap_err(),
        Error::Kernel(KernelError::WouldBlock)
    );
    let mut first_request = server.receive(WaitMode::Poll).unwrap();
    let mut second_request = server.receive(WaitMode::Poll).unwrap();
    let first_reply = first_request.take_reply().unwrap();
    let second_reply = second_request.take_reply().unwrap();
    second_reply
        .send(&mut Message::new(22, &[]).unwrap())
        .unwrap();
    let items = [&first, &second].map(|call| WaitItem::new(call, ipc::READABLE));
    assert_eq!(ipc::wait_many(&items, WaitMode::Poll).unwrap().index, 1);
    assert_eq!(
        second.cancel(),
        Err(Error::Kernel(KernelError::InvalidState))
    );
    assert_eq!(second.try_receive().unwrap().tag, 22);
    let mut response = Message::new(11, &[]).unwrap();
    response
        .push(Transfer {
            handle: first_request.take_handle(0).unwrap(),
            rights: Rights::WAIT,
        })
        .unwrap();
    first_reply.send(&mut response).unwrap();
    let mut response = first.try_receive().unwrap();
    assert_eq!(response.tag, 11);
    let event = response
        .take_handle(0)
        .unwrap()
        .downcast::<runtime::Event>()
        .unwrap();
    assert_eq!(event.info().unwrap().rights, Rights::WAIT);
    assert_eq!(
        first.try_receive().unwrap_err(),
        Error::Kernel(KernelError::InvalidState)
    );
}

fn cancelled_calls() {
    let rights = Rights::READ_WRITE | Rights::WAIT;
    let (client, server) = process::task().channel(rights, rights).unwrap();
    let cancelled = client
        .start_call(&mut Message::new(3, &[]).unwrap())
        .unwrap();
    let mut request = server.receive(WaitMode::Poll).unwrap();
    let reply = request.take_reply().unwrap();
    cancelled.cancel().unwrap();
    assert_eq!(
        cancelled.try_receive().unwrap_err(),
        Error::Kernel(KernelError::Cancelled)
    );
    assert_eq!(
        reply.send(&mut Message::new(33, &[]).unwrap()),
        Err(Error::Kernel(KernelError::InvalidState))
    );
    let closed = client
        .start_call(&mut Message::new(4, &[]).unwrap())
        .unwrap();
    drop(closed);
    let mut request = server.receive(WaitMode::Poll).unwrap();
    let reply = request.take_reply().unwrap();
    assert_eq!(
        reply.send(&mut Message::new(44, &[]).unwrap()),
        Err(Error::Kernel(KernelError::InvalidState))
    );
    let failed = client
        .start_call(&mut Message::new(5, &[]).unwrap())
        .unwrap();
    let worker = thread::spawn(move || {
        let items = [WaitItem::new(&failed, ipc::READABLE)];
        ipc::wait_many(&items, WaitMode::Forever).unwrap();
        failed.try_receive().unwrap_err()
    })
    .unwrap();
    super::wait_for(|| worker.thread().state().unwrap().0 == ThreadState::Blocked);
    drop(server);
    assert_eq!(
        worker.join().unwrap(),
        Error::Kernel(KernelError::ServerGone)
    );
}

fn call_backpressure() {
    let rights = Rights::READ_WRITE | Rights::WAIT;
    let (client, server) = process::task().channel(rights, rights).unwrap();
    for _ in 0..32 {
        client
            .send(&mut Message::new(0, &[]).unwrap(), WaitMode::Poll)
            .unwrap();
    }
    let event = process::task()
        .event(Rights::TRANSFER | Rights::WAIT)
        .unwrap();
    let mut request = Message::new(1, &[]).unwrap();
    request
        .push(Transfer {
            handle: event.into(),
            rights: Rights::WAIT,
        })
        .unwrap();
    assert_eq!(
        client.start_call(&mut request).unwrap_err(),
        Error::Kernel(KernelError::WouldBlock)
    );
    assert_eq!(request.transfers().len(), 1);
    server.receive(WaitMode::Poll).unwrap();
    let pending = client.start_call(&mut request).unwrap();
    assert!(request.transfers().is_empty());
    drop(server);
    assert_eq!(
        pending.try_receive().unwrap_err(),
        Error::Kernel(KernelError::ServerGone)
    );
}
