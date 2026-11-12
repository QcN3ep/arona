// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use alloc::{vec, vec::Vec};
use lwip::{Endpoint, Error, SocketKind, Stack};

pub(super) fn run(stack: &Stack) {
    let mut receiver = stack.socket(SocketKind::Udp).unwrap();
    receiver.bind(Endpoint::loopback(0)).unwrap();
    let address = receiver.local_endpoint().unwrap();
    let mut sender = stack.socket(SocketKind::Udp).unwrap();
    sender.bind(Endpoint::loopback(0)).unwrap();
    let source = sender.local_endpoint().unwrap();
    let payload = vec![91; 60 * 1024];
    let mut output = vec![0; payload.len()];
    let mut blocks = Vec::new();
    blocks.try_reserve_exact(1024).unwrap();
    let mut spare = Vec::<u8>::new();
    spare.try_reserve_exact(16 * 1024).unwrap();
    for size in [1024 * 1024, 32 * 1024] {
        loop {
            let mut block = Vec::<u8>::new();
            if block.try_reserve_exact(size).is_err() {
                break;
            }
            blocks.push(block);
            assert!(blocks.len() < 1024);
        }
    }
    // Leave space for small synchronization allocations while packet storage fails.
    drop(spare);
    assert_eq!(sender.send_to(&payload, address), Err(Error::NoMemory));
    assert_eq!(receiver.receive(&mut output), Err(Error::WouldBlock));
    drop(blocks);
    assert_eq!(
        super::retry(|| sender.send_to(&payload, address)),
        payload.len()
    );
    let (count, peer) = super::retry(|| receiver.receive_from(&mut output));
    assert_eq!(peer, source);
    assert_eq!(count, payload.len());
    assert_eq!(output, payload);
}
