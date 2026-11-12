// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

mod entropy;
mod memory;
mod tcp;
mod udp;

use lwip::{Error, Stack};
use runtime::{
    DeviceSet,
    ipc::{Message, WaitMode},
    process, thread,
};

fn main() {
    let bootstrap = process::bootstrap().unwrap();
    let devices = bootstrap
        .receive(WaitMode::Poll)
        .unwrap()
        .take_handle(0)
        .unwrap()
        .downcast::<DeviceSet>()
        .unwrap();
    let seed = entropy::read(&devices);
    let stack = Stack::initialize(seed).unwrap();
    assert!(matches!(
        Stack::initialize(seed),
        Err(Error::AlreadyInitialized)
    ));
    tcp::run(&stack);
    udp::run(&stack);
    memory::run(&stack);
    bootstrap
        .send(&mut Message::new(73, &[2]).unwrap(), WaitMode::Forever)
        .unwrap();
}

fn retry<T>(mut attempt: impl FnMut() -> Result<T, Error>) -> T {
    let deadline = thread::now_ns().unwrap() + 5_000_000_000;
    loop {
        match attempt() {
            Ok(value) => return value,
            Err(Error::WouldBlock | Error::InProgress | Error::NotConnected) => {}
            Err(error) => panic!("socket operation failed: {error:?}"),
        }
        assert!(
            thread::now_ns().unwrap() < deadline,
            "socket operation timed out"
        );
        thread::sleep_until(thread::now_ns().unwrap() + 1_000_000).unwrap();
    }
}
