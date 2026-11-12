// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use lwip::{Endpoint, Error, Shutdown, Socket, SocketKind, Stack};
use runtime::thread;

const LENGTH: usize = 64 * 1024;

pub(super) fn run(stack: &Stack) {
    let mut listener = stack.socket(SocketKind::Tcp).unwrap();
    listener.bind(Endpoint::loopback(0)).unwrap();
    let address = listener.local_endpoint().unwrap();
    listener.listen(4).unwrap();
    assert!(matches!(listener.accept(), Err(Error::WouldBlock)));
    let mut client = stack.socket(SocketKind::Tcp).unwrap();
    let worker = thread::spawn(move || {
        assert!(matches!(
            client.connect(address),
            Ok(()) | Err(Error::InProgress)
        ));
        assert_eq!(super::retry(|| client.peer_endpoint()), address);
        transfer(&mut client, 19, false);
        transfer(&mut client, 53, true);
        assert_eq!(super::retry(|| client.receive(&mut [0; 1])), 0);
        client.shutdown(Shutdown::Write).unwrap();
    })
    .unwrap();
    let (mut accepted, peer) = super::retry(|| listener.accept());
    assert_eq!(accepted.peer_endpoint().unwrap(), peer);
    assert_eq!(accepted.local_endpoint().unwrap(), address);
    transfer(&mut accepted, 19, true);
    transfer(&mut accepted, 53, false);
    accepted.shutdown(Shutdown::Write).unwrap();
    assert_eq!(super::retry(|| accepted.receive(&mut [0; 1])), 0);
    worker.join().unwrap();
    drop(accepted);
    drop(listener);
    refused(stack);
}

fn transfer(socket: &mut Socket, offset: u8, receive: bool) {
    let mut bytes = [0; 4096];
    let mut total = 0;
    while total < LENGTH {
        let size = bytes.len().min(LENGTH - total);
        let count = if receive {
            let count = super::retry(|| socket.receive(&mut bytes[..size]));
            assert!(count > 0);
            for (index, byte) in bytes[..count].iter().enumerate() {
                assert_eq!(*byte, (total + index) as u8 ^ offset);
            }
            count
        } else {
            for (index, byte) in bytes.iter_mut().enumerate() {
                *byte = (total + index) as u8 ^ offset;
            }
            super::retry(|| socket.send(&bytes[..size]))
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
    drop(unused);
    let mut socket = stack.socket(SocketKind::Tcp).unwrap();
    // lwIP reports a reset when loopback rejects a SYN for an unbound port.
    match socket.connect(address) {
        Err(Error::ConnectionReset) => {}
        Err(Error::InProgress) => {
            super::retry(|| match socket.take_error() {
                Err(Error::ConnectionReset) => Ok(()),
                Ok(()) | Err(Error::InProgress) => Err(Error::WouldBlock),
                Err(error) => panic!("unexpected connection error: {error:?}"),
            });
        }
        result => panic!("unexpected refused connect result: {result:?}"),
    }
    let mut reusable = stack.socket(SocketKind::Tcp).unwrap();
    reusable.bind(address).unwrap();
    reusable.listen(1).unwrap();
}
