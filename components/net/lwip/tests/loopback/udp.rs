// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use lwip::{Endpoint, Error, SocketKind, Stack};

pub(super) fn run(stack: &Stack) {
    let mut server = stack.socket(SocketKind::Udp).unwrap();
    server.bind(Endpoint::loopback(0)).unwrap();
    let address = server.local_endpoint().unwrap();
    let mut duplicate = stack.socket(SocketKind::Udp).unwrap();
    assert_eq!(duplicate.bind(address), Err(Error::AddressInUse));
    duplicate.bind(Endpoint::loopback(0)).unwrap();
    assert_eq!(duplicate.receive(&mut [0; 1]), Err(Error::WouldBlock));

    let mut sender = stack.socket(SocketKind::Udp).unwrap();
    sender.bind(Endpoint::loopback(0)).unwrap();
    let peer = sender.local_endpoint().unwrap();
    let payload = [73; 4096];
    for packet in [&payload[..], &[][..], b"third".as_slice()] {
        assert_eq!(
            super::retry(|| sender.send_to(packet, address)),
            packet.len()
        );
    }
    let mut output = [0; 8192];
    for expected in [&payload[..], &[][..], b"third".as_slice()] {
        let (count, source) = super::retry(|| server.receive_from(&mut output));
        assert_eq!(&output[..count], expected);
        assert_eq!(source, peer);
    }
    assert_eq!(server.receive(&mut output), Err(Error::WouldBlock));
    sender.connect(address).unwrap();
    assert_eq!(sender.peer_endpoint().unwrap(), address);
    assert_eq!(super::retry(|| sender.send(b"connected")), 9);
    let (count, source) = super::retry(|| server.receive_from(&mut output));
    assert_eq!(&output[..count], b"connected");
    assert_eq!(source, peer);
    server.send_to(b"reply", peer).unwrap();
    assert_eq!(super::retry(|| sender.receive(&mut output)), 5);
    assert_eq!(&output[..5], b"reply");
    drop(server);
    let mut reused = stack.socket(SocketKind::Udp).unwrap();
    reused.bind(address).unwrap();
}
