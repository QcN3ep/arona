// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use alloc::sync::Arc;
use lwip::{Endpoint, Error, Readiness, Shutdown, SocketKind, Stack, Subscription};
use runtime::{
    ipc::{self, WaitItem, WaitMode},
    sync::Mailbox,
    thread,
};

pub(super) fn attempt<T>(
    changes: &mut Subscription,
    mut operation: impl FnMut() -> Result<T, Error>,
) -> T {
    let deadline = deadline();
    loop {
        let observed = changes.snapshot();
        match operation() {
            Ok(value) => return value,
            Err(Error::WouldBlock) => {
                changes.wait(observed, WaitMode::Until(deadline)).unwrap();
            }
            Err(error) => panic!("socket operation failed: {error:?}"),
        }
    }
}

pub(super) fn ready(
    changes: &mut Subscription,
    predicate: impl Fn(Readiness) -> bool,
) -> Readiness {
    let deadline = deadline();
    loop {
        let state = changes.snapshot();
        if predicate(state) {
            return state;
        }
        changes.wait(state, WaitMode::Until(deadline)).unwrap();
    }
}

fn deadline() -> u64 {
    thread::now_ns().unwrap() + 30_000_000_000
}

pub(super) fn run(stack: &Stack) {
    subscribers(stack);
    capacity_and_close(stack);
    shutdown(stack);
}

fn subscribers(stack: &Stack) {
    let mut receiver = stack.socket(SocketKind::Udp).unwrap();
    receiver.bind(Endpoint::loopback(0)).unwrap();
    let address = receiver.local_endpoint().unwrap();
    let first = receiver.subscribe().unwrap();
    let second = receiver.subscribe().unwrap();
    let registered = Arc::new(Mailbox::new(2).unwrap());
    let mut workers = alloc::vec::Vec::new();
    for changes in [first, second] {
        let registered = Arc::clone(&registered);
        workers.push(
            thread::spawn(move || {
                assert!(!changes.snapshot().readable);
                registered.send((), WaitMode::Forever).unwrap();
                // Waiting directly on each Event also checks broadcast delivery.
                ipc::wait_many(
                    &[WaitItem::new(changes.event(), 1)],
                    WaitMode::Until(deadline()),
                )
                .unwrap();
                assert!(changes.snapshot().readable);
                changes
            })
            .unwrap(),
        );
    }
    for _ in 0..2 {
        registered.receive(WaitMode::Forever).unwrap();
    }
    let mut sender = stack.socket(SocketKind::Udp).unwrap();
    sender.send_to(b"one", address).unwrap();
    let mut changes = workers.remove(0).join().unwrap();
    let other = workers.remove(0).join().unwrap();
    assert_eq!(receiver.receive(&mut [0; 1]), Ok(1));
    assert!(!changes.snapshot().readable);
    let observed = changes.snapshot();
    sender.send_to(b"two", address).unwrap();
    // A change after the snapshot survives any older, unconsumed Event bit.
    assert!(
        changes
            .wait(observed, WaitMode::Until(deadline()))
            .unwrap()
            .readable
    );
    let mut bytes = [0; 8];
    assert_eq!(receiver.receive(&mut bytes), Ok(3));
    assert_eq!(&bytes[..3], b"two");
    let observed = changes.snapshot();
    assert_eq!(
        changes.wait(observed, WaitMode::Poll),
        Err(Error::WouldBlock)
    );
    // Register after data arrival and keep observing partial/empty datagrams.
    sender.send_to(b"", address).unwrap();
    ready(&mut changes, |state| state.readable);
    let late = receiver.subscribe().unwrap();
    assert!(late.snapshot().readable);
    assert_eq!(receiver.receive(&mut bytes), Ok(0));
    assert!(!late.snapshot().read_closed);
    receiver.close().wait(WaitMode::Forever).unwrap();
    let closed = changes.snapshot();
    assert!(closed.closed);
    assert!(other.snapshot().closed);
    let mut replacement = stack.socket(SocketKind::Udp).unwrap();
    replacement.bind(address).unwrap();
    let mut replacement_changes = replacement.subscribe().unwrap();
    sender.send_to(b"new", address).unwrap();
    assert_eq!(
        attempt(&mut replacement_changes, || replacement.receive(&mut bytes)),
        3
    );
    assert_eq!(&bytes[..3], b"new");
    assert_eq!(changes.snapshot(), closed);
    replacement.close().wait(WaitMode::Forever).unwrap();
    sender.close().wait(WaitMode::Forever).unwrap();
}

fn pair(stack: &Stack) -> (lwip::Socket, lwip::Socket) {
    let mut listener = stack.socket(SocketKind::Tcp).unwrap();
    listener.bind(Endpoint::loopback(0)).unwrap();
    listener.listen(1).unwrap();
    let mut incoming = listener.subscribe().unwrap();
    let mut client = stack.socket(SocketKind::Tcp).unwrap();
    let mut connected = client.subscribe().unwrap();
    assert!(matches!(
        client.connect(listener.local_endpoint().unwrap()),
        Ok(()) | Err(Error::InProgress)
    ));
    let (server, _) = attempt(&mut incoming, || listener.accept());
    ready(&mut connected, |state| state.writable || state.error);
    client.take_error().unwrap();
    listener.close().wait(WaitMode::Forever).unwrap();
    (client, server)
}

fn capacity_and_close(stack: &Stack) {
    let (mut sender, mut receiver) = pair(stack);
    let (mut first, mut second) = pair(stack);
    let mut received = second.subscribe().unwrap();
    let mut changes = sender.subscribe().unwrap();
    let payload = [67; 4096];
    let mut total = 0;
    loop {
        match sender.send(&payload) {
            Ok(count) => total += count,
            Err(Error::WouldBlock) => break,
            result => panic!("unexpected send under pressure: {result:?}"),
        }
    }
    assert!(!changes.snapshot().writable);
    let worker = thread::spawn(move || {
        let mut changes = receiver.subscribe().unwrap();
        let mut remaining = total;
        let mut bytes = [0; 4096];
        while remaining != 0 {
            let size = bytes.len().min(remaining);
            let count = attempt(&mut changes, || receiver.receive(&mut bytes[..size]));
            assert!(count > 0);
            assert!(bytes[..count].iter().all(|byte| *byte == 67));
            remaining -= count;
        }
        assert_eq!(attempt(&mut changes, || receiver.receive(&mut bytes)), 4);
        assert_eq!(&bytes[..4], b"tail");
        assert_eq!(attempt(&mut changes, || receiver.receive(&mut bytes)), 0);
        receiver.close().wait(WaitMode::Forever).unwrap();
    })
    .unwrap();
    ready(&mut changes, |state| state.writable);
    assert_eq!(attempt(&mut changes, || sender.send(b"tail")), 4);
    let mut closing = sender.close();
    // Transfer on an independent connection before waiting for final close.
    first.send(b"progress").unwrap();
    assert_eq!(attempt(&mut received, || second.receive(&mut [0; 8])), 8);
    closing.wait(WaitMode::Until(deadline())).unwrap();
    worker.join().unwrap();
    first.close().wait(WaitMode::Forever).unwrap();
    second.close().wait(WaitMode::Forever).unwrap();
}

fn shutdown(stack: &Stack) {
    let (client, mut server) = pair(stack);
    let mut eof = server.subscribe().unwrap();
    let mut pending = client.shutdown(Shutdown::Write);
    let (mut client, result) = match pending.wait(WaitMode::Poll) {
        Ok(result) => result,
        Err(Error::WouldBlock) => {
            ipc::wait_many(
                &[WaitItem::new(pending.event().unwrap(), 1)],
                WaitMode::Until(deadline()),
            )
            .unwrap();
            pending.wait(WaitMode::Forever).unwrap()
        }
        Err(error) => panic!("unexpected shutdown wait error: {error:?}"),
    };
    assert!(pending.event().is_none());
    result.unwrap();
    assert!(client.readiness().write_closed);
    assert_eq!(attempt(&mut eof, || server.receive(&mut [0; 8])), 0);
    server.send(b"reply").unwrap();
    let mut changes = client.subscribe().unwrap();
    let mut bytes = [0; 8];
    assert_eq!(attempt(&mut changes, || client.receive(&mut bytes[..2])), 2);
    assert_eq!(&bytes[..2], b"re");
    assert!(client.readiness().readable);
    assert_eq!(client.receive(&mut bytes), Ok(3));
    assert_eq!(&bytes[..3], b"ply");
    let mut closed = client.shutdown(Shutdown::Read).close();
    closed.wait(WaitMode::Forever).unwrap();
    assert!(changes.snapshot().closed);
    server.close().wait(WaitMode::Forever).unwrap();

    let (client, mut server) = pair(stack);
    let mut changes = client.subscribe().unwrap();
    drop(client.shutdown(Shutdown::Write));
    ready(&mut changes, |state| state.closed);
    assert_eq!(super::retry(|| server.receive(&mut bytes)), 0);
    server.close().wait(WaitMode::Forever).unwrap();

    let socket = stack.socket(SocketKind::Udp).unwrap();
    let (socket, result) = socket
        .shutdown(Shutdown::Write)
        .wait(WaitMode::Forever)
        .unwrap();
    assert_eq!(result, Err(Error::Other(95)));
    socket.close().wait(WaitMode::Forever).unwrap();
}
