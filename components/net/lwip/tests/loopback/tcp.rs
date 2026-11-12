// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use lwip::{Endpoint, Error, Shutdown, Socket, SocketKind, Stack};
use runtime::ipc::WaitMode;
use runtime::thread;

const LENGTH: usize = 64 * 1024;

pub(super) fn run(stack: &Stack) {
    let mut listener = stack.socket(SocketKind::Tcp).unwrap();
    listener.bind(Endpoint::loopback(0)).unwrap();
    let address = listener.local_endpoint().unwrap();
    listener.listen(4).unwrap();
    let mut incoming = listener.subscribe().unwrap();
    assert!(matches!(listener.accept(), Err(Error::WouldBlock)));
    let mut client = stack.socket(SocketKind::Tcp).unwrap();
    let worker = thread::spawn(move || {
        let mut changes = client.subscribe().unwrap();
        assert!(matches!(
            client.connect(address),
            Ok(()) | Err(Error::InProgress)
        ));
        super::readiness::ready(&mut changes, |state| state.writable || state.error);
        client.take_error().unwrap();
        assert_eq!(client.peer_endpoint().unwrap(), address);
        transfer(&mut client, 19, false);
        transfer(&mut client, 53, true);
        assert_eq!(
            super::readiness::attempt(&mut changes, || client.receive(&mut [0; 1])),
            0
        );
        let (client, result) = client
            .shutdown(Shutdown::Write)
            .wait(WaitMode::Forever)
            .unwrap();
        result.unwrap();
        client.close().wait(WaitMode::Forever).unwrap();
    })
    .unwrap();
    let (mut accepted, peer) = super::readiness::attempt(&mut incoming, || listener.accept());
    let mut changes = accepted.subscribe().unwrap();
    assert_eq!(accepted.peer_endpoint().unwrap(), peer);
    assert_eq!(accepted.local_endpoint().unwrap(), address);
    transfer(&mut accepted, 19, true);
    transfer(&mut accepted, 53, false);
    let (mut accepted, result) = accepted
        .shutdown(Shutdown::Write)
        .wait(WaitMode::Forever)
        .unwrap();
    result.unwrap();
    assert!(accepted.readiness().write_closed);
    assert!(!accepted.readiness().writable);
    assert_eq!(
        super::readiness::attempt(&mut changes, || accepted.receive(&mut [0; 1])),
        0
    );
    assert!(accepted.readiness().readable);
    assert!(accepted.readiness().read_closed);
    worker.join().unwrap();
    accepted.close().wait(WaitMode::Forever).unwrap();
    listener.close().wait(WaitMode::Forever).unwrap();
    refused(stack);
}

fn transfer(socket: &mut Socket, offset: u8, receive: bool) {
    let mut changes = socket.subscribe().unwrap();
    let mut bytes = [0; 4096];
    let mut total = 0;
    while total < LENGTH {
        let size = bytes.len().min(LENGTH - total);
        let count = if receive {
            let count =
                super::readiness::attempt(&mut changes, || socket.receive(&mut bytes[..size]));
            assert!(count > 0);
            for (index, byte) in bytes[..count].iter().enumerate() {
                assert_eq!(*byte, (total + index) as u8 ^ offset);
            }
            count
        } else {
            for (index, byte) in bytes.iter_mut().enumerate() {
                *byte = (total + index) as u8 ^ offset;
            }
            super::readiness::attempt(&mut changes, || socket.send(&bytes[..size]))
        };
        assert!(count > 0);
        total += count;
    }
    assert_eq!(total, LENGTH);
}

fn refused(stack: &Stack) {
    let mut unused = stack.socket(SocketKind::Tcp).unwrap();
    unused.bind(Endpoint::loopback(0)).unwrap();
    let address = unused.local_endpoint().unwrap();
    unused.close().wait(WaitMode::Forever).unwrap();
    let mut socket = stack.socket(SocketKind::Tcp).unwrap();
    let mut changes = socket.subscribe().unwrap();
    // lwIP reports a reset when loopback rejects a SYN for an unbound port.
    assert!(matches!(
        socket.connect(address),
        Err(Error::InProgress | Error::ConnectionReset)
    ));
    super::readiness::ready(&mut changes, |state| state.error);
    assert!(changes.snapshot().error);
    assert!(socket.readiness().error);
    assert_eq!(socket.take_error(), Err(Error::ConnectionReset));
    assert_eq!(socket.take_error(), Ok(()));
    let mut reusable = stack.socket(SocketKind::Tcp).unwrap();
    reusable.bind(address).unwrap();
    reusable.listen(1).unwrap();
}
