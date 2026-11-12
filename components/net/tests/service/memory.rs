// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::until;
use net::{Client, Completion, Endpoint, Error, Request, SocketKind};
use runtime::{FrameAllocation, Handle, Rights, process};

pub fn run(client: &Client<'_>) {
    let mut receiver = client.socket(SocketKind::Udp, until()).unwrap();
    receiver
        .perform(Request::Bind(Endpoint::loopback(0)), until())
        .unwrap();
    let target = match receiver.perform(Request::LocalEndpoint, until()).unwrap() {
        Completion::Endpoint(target) => target,
        _ => panic!("missing address"),
    };
    let mut sender = client.socket(SocketKind::Udp, until()).unwrap();
    sender
        .perform(Request::SendTo(b"keep", target), until())
        .unwrap();
    // Reserve bookkeeping and a little protocol progress space before exhausting
    // real frames. The receive result needs more space than this remaining reserve.
    let mut frames: alloc::vec::Vec<Handle<FrameAllocation>> = alloc::vec::Vec::new();
    frames.try_reserve_exact(4096).unwrap();
    let spare = process::task().allocate(8, Rights::READ).unwrap();
    for count in [1024, 64, 16, 1] {
        loop {
            match process::task().allocate(count, Rights::READ) {
                Ok(allocation) => {
                    assert!(frames.len() < frames.capacity());
                    frames.push(allocation);
                }
                Err(runtime::Error::Kernel(runtime::KernelError::NoMemory)) => break,
                Err(error) => panic!("unexpected allocation error: {error:?}"),
            }
        }
    }
    drop(spare);
    assert!(matches!(
        receiver.perform(Request::ReceiveDatagram, until()),
        Err(Error::NoMemory)
    ));
    drop(frames);
    let completion = if receiver.has_pending() {
        receiver.complete(until())
    } else {
        receiver.perform(Request::ReceiveDatagram, until())
    };
    let data = match completion {
        Ok(Completion::Data(data)) => data,
        Err(Error::NoMemory) => {
            match receiver.perform(Request::ReceiveDatagram, until()).unwrap() {
                Completion::Data(data) => data,
                _ => panic!("missing retained datagram"),
            }
        }
        _ => panic!("lost datagram after allocation failure"),
    };
    let mut bytes = [0; 4];
    data.read(0, &mut bytes).unwrap();
    assert_eq!(&bytes, b"keep");
    assert!(matches!(
        receiver.perform(Request::ReceiveDatagram, until()),
        Err(Error::WouldBlock)
    ));
    for socket in [&mut sender, &mut receiver] {
        socket.perform(Request::Close, until()).unwrap();
    }
}
