// SPDX-License-Identifier: MPL-2.0

#![no_std]
#![feature(thread_local)]

extern crate alloc;

use alloc::{boxed::Box, vec, vec::Vec};
use core::cell::{Cell, RefCell};
use runtime::{Channel, Handle, ipc::Message, process};

#[thread_local]
static INITIALIZED: Cell<usize> = Cell::new(0x12345678);
#[thread_local]
static ZERO: Cell<usize> = Cell::new(0);
#[thread_local]
static ALIGNED: Aligned = Aligned([0x6b; 8192]);

runtime::thread_local! {
    static REPORT: Report = Report(process::bootstrap().unwrap());
    static VALUES: RefCell<Vec<usize>> = RefCell::new(vec![3, 5, 8]);
    static RECURSIVE: usize = {
        assert_eq!(RECURSIVE.try_with(|_| ()), Err(runtime::tls::AccessError::Initializing));
        7
    };
}

struct Report(Handle<Channel>);

impl Drop for Report {
    fn drop(&mut self) {
        assert_eq!(
            REPORT.try_with(|_| ()),
            Err(runtime::tls::AccessError::Destroyed)
        );
        assert_eq!(
            VALUES.try_with(|_| ()),
            Err(runtime::tls::AccessError::Destroyed)
        );
        let mut message = Message::new(73, &[INITIALIZED.get() as u64, ZERO.get() as u64]).unwrap();
        self.0
            .send(&mut message, runtime::ipc::WaitMode::Forever)
            .unwrap();
    }
}

#[repr(align(8192))]
struct Aligned([u8; 8192]);

fn main() {
    assert_eq!(INITIALIZED.get(), 0x12345678);
    assert_eq!(ZERO.get(), 0);
    INITIALIZED.set(37);
    ZERO.set(41);
    REPORT.with(|_| ());
    let mode = REPORT.with(|report| report.0.receive(runtime::ipc::WaitMode::Poll).unwrap().tag);
    match mode {
        0 => {}
        1 => panic!("intentional runtime integration test failure"),
        2 => {
            core::hint::black_box(Vec::<u8>::with_capacity(core::hint::black_box(
                isize::MAX as usize,
            )));
        }
        _ => panic!("unknown runtime test scenario"),
    }
    assert_eq!(core::ptr::addr_of!(ALIGNED) as usize % 8192, 0);
    assert!(ALIGNED.0.iter().all(|byte| *byte == 0x6b));
    assert_eq!(RECURSIVE.with(|value| *value), 7);
    VALUES.with(|values| {
        let mut values = values.borrow_mut();
        values.extend(0..100_000);
        assert_eq!(values.len(), 100_003);
        assert_eq!(values.last(), Some(&99_999));
    });
    let mut blocks = Vec::new();
    for index in 0..128 {
        blocks.push(vec![index as u8; index * 37 + 1]);
    }
    for (index, block) in blocks.iter().enumerate() {
        assert!(block.iter().all(|byte| *byte == index as u8));
    }
    drop(blocks);
    for _ in 0..8 {
        let mut large = vec![0u8; 1024 * 1024];
        assert!(large.iter().all(|byte| *byte == 0));
        large.fill(0xa5);
        let aligned = Box::new(Aligned([0x5a; 8192]));
        assert_eq!(core::ptr::from_ref(&*aligned) as usize % 8192, 0);
        assert!(aligned.0.iter().all(|byte| *byte == 0x5a));
    }
    assert_eq!(
        process::task().info().unwrap().kind,
        runtime::ObjectType::Task
    );
    runtime::thread::yield_now().unwrap();
}
