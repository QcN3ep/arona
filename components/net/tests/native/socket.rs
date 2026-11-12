// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use net::{Completion, Data, Error, Request, Socket, Subscription};
use runtime::ipc::WaitMode;

pub fn deadline() -> u64 {
    runtime::thread::now_ns().unwrap() + 60_000_000_000
}
pub fn until() -> WaitMode {
    WaitMode::Until(deadline())
}

pub fn send_pattern(socket: &mut Socket<'_>, notice: &Subscription, length: usize) {
    let bytes: alloc::vec::Vec<_> = (0..length).map(|index| (index % 251) as u8).collect();
    let mut offset = 0;
    while offset < length {
        let observed = notice.snapshot(deadline()).unwrap();
        match socket.perform(
            Request::Send(&bytes[offset..length.min(offset + 65536)]),
            until(),
        ) {
            Ok(Completion::Count(count)) => {
                assert!(count > 0);
                offset += count;
            }
            Err(Error::WouldBlock) => {
                notice.wait(observed, until()).unwrap();
            }
            _ => panic!("stream send failed"),
        }
    }
}
pub fn receive(socket: &mut Socket<'_>, notice: &Subscription, datagram: bool) -> Data {
    loop {
        let observed = notice.snapshot(deadline()).unwrap();
        let request = if datagram {
            Request::ReceiveDatagram
        } else {
            Request::Receive(8192)
        };
        match socket.perform(request, until()) {
            Ok(Completion::Data(data)) => return data,
            Err(Error::WouldBlock) => {
                notice.wait(observed, until()).unwrap();
            }
            _ => panic!("receive failed"),
        }
    }
}
pub fn receive_pattern(socket: &mut Socket<'_>, notice: &Subscription, length: usize) {
    let mut offset = 0;
    while offset < length {
        let data = receive(socket, notice, false);
        assert!(!data.is_empty());
        let mut bytes = alloc::vec![0; data.len()];
        data.read(0, &mut bytes).unwrap();
        for byte in bytes {
            assert_eq!(byte, (offset % 251) as u8);
            offset += 1;
        }
    }
    assert_eq!(offset, length);
}
