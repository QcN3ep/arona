// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use alloc::{sync::Arc, vec::Vec};
use lwip::{Endpoint, Error, SocketKind, Stack};
use runtime::ipc::WaitMode;
use runtime::thread;

pub(super) fn run(stack: &Arc<Stack>) {
    let mut listener = stack.socket(SocketKind::Tcp).unwrap();
    listener.bind(Endpoint::loopback(0)).unwrap();
    listener.listen(1).unwrap();
    let mut client = stack.socket(SocketKind::Tcp).unwrap();
    assert!(matches!(
        client.connect(listener.local_endpoint().unwrap()),
        Ok(()) | Err(Error::InProgress)
    ));
    super::retry(|| client.send(b"before accept"));
    let (mut accepted, _) = super::retry(|| listener.accept());
    let mut pending = accepted.subscribe().unwrap();
    super::readiness::ready(&mut pending, |state| state.readable);

    let other = Arc::clone(stack);
    let worker = thread::spawn(move || population(&other)).unwrap();
    population(stack);
    worker.join().unwrap();

    let mut output = [0; 32];
    assert_eq!(super::retry(|| accepted.receive(&mut output)), 13);
    assert_eq!(&output[..13], b"before accept");
    assert_eq!(super::retry(|| accepted.send(b"after growth")), 12);
    assert_eq!(super::retry(|| client.receive(&mut output)), 12);
    assert_eq!(&output[..12], b"after growth");
}

fn population(stack: &Stack) {
    let mut sender = stack.socket(SocketKind::Udp).unwrap();
    sender.bind(Endpoint::loopback(0)).unwrap();
    let source = sender.local_endpoint().unwrap();
    let mut receivers = Vec::new();
    for _ in 0..64 {
        let mut receiver = stack.socket(SocketKind::Udp).unwrap();
        receiver.bind(Endpoint::loopback(0)).unwrap();
        let address = receiver.local_endpoint().unwrap();
        receivers.push((Some(receiver), address));
    }

    for round in 0..3 {
        for (index, (_, address)) in receivers.iter().enumerate() {
            let payload = [round, index as u8];
            assert_eq!(super::retry(|| sender.send_to(&payload, *address)), 2);
        }
        for (index, (receiver, address)) in receivers.iter_mut().enumerate() {
            if index % 2 == 0 {
                // Closing discards queued data before a replacement binds the same port.
                receiver
                    .take()
                    .unwrap()
                    .close()
                    .wait(WaitMode::Forever)
                    .unwrap();
                let mut replacement = stack.socket(SocketKind::Udp).unwrap();
                replacement.bind(*address).unwrap();
                assert_eq!(replacement.receive(&mut [0; 2]), Err(Error::WouldBlock));
                *receiver = Some(replacement);
                assert_eq!(
                    super::retry(|| sender.send_to(&[round, index as u8], *address)),
                    2
                );
            }
            let mut payload = [0; 2];
            let (length, peer) =
                super::retry(|| receiver.as_mut().unwrap().receive_from(&mut payload));
            assert_eq!(length, 2);
            assert_eq!(peer, source);
            assert_eq!(payload, [round, index as u8]);
            assert_eq!(
                receiver.as_mut().unwrap().receive(&mut payload),
                Err(Error::WouldBlock)
            );
        }
    }
}
