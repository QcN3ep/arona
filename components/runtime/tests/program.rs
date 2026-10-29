// SPDX-License-Identifier: MPL-2.0

#![no_std]
#![forbid(unsafe_code)]
#![feature(thread_local)]

extern crate alloc;

use alloc::{boxed::Box, sync::Arc, vec, vec::Vec};
use core::{
    cell::{Cell, RefCell},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
use runtime::{Channel, Handle, ipc::Message, thread};

mod ipc;
mod logger;
mod process;

static TLS_DROPS: AtomicUsize = AtomicUsize::new(0);

#[thread_local]
static INITIALIZED: Cell<usize> = Cell::new(0x12345678);
#[thread_local]
static ZERO: Cell<usize> = Cell::new(0);
#[thread_local]
static ALIGNED: Aligned = Aligned([0x6b; 8192]);

runtime::thread_local! {
    static REPORT: Report = Report(runtime::process::bootstrap().unwrap());
    static VALUES: RefCell<Vec<usize>> = RefCell::new(vec![3, 5, 8]);
    static RECURSIVE: usize = {
        assert_eq!(RECURSIVE.try_with(|_| ()), Err(runtime::tls::AccessError::Initializing));
        7
    };
    static WORKER_DROP: WorkerDrop = WorkerDrop(ZERO.get());
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

struct CountDrop(Arc<AtomicUsize>);

impl Drop for CountDrop {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct WorkerDrop(usize);

impl Drop for WorkerDrop {
    fn drop(&mut self) {
        assert_eq!(ZERO.get(), self.0);
        TLS_DROPS.fetch_add(1, Ordering::SeqCst);
    }
}

fn main() {
    assert_eq!(INITIALIZED.get(), 0x12345678);
    assert_eq!(ZERO.get(), 0);
    INITIALIZED.set(37);
    ZERO.set(41);
    REPORT.with(|_| ());
    let startup = REPORT.with(|report| report.0.receive(runtime::ipc::WaitMode::Poll).unwrap());
    let mode = startup.tag;
    match mode {
        0 => logger::run(startup),
        4 => {}
        1 => panic!("intentional runtime integration test failure"),
        2 => {
            core::hint::black_box(Vec::<u8>::with_capacity(core::hint::black_box(
                isize::MAX as usize,
            )));
        }
        3 => {
            thread::spawn(|| panic!("intentional worker panic"))
                .unwrap()
                .join()
                .unwrap();
        }
        7 | 8 => {
            VALUES.with(|_| ());
            if mode == 7 {
                process::run(startup);
            } else {
                process::child(startup);
            }
            return;
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
        runtime::process::task().info().unwrap().kind,
        runtime::ObjectType::Task
    );
    runtime::thread::yield_now().unwrap();
    if mode == 4 {
        let dropped = Arc::new(AtomicUsize::new(0));
        let capture = CountDrop(dropped.clone());
        assert!(matches!(
            thread::spawn(move || drop(capture)),
            Err(runtime::Error::Kernel(abi::Error::Denied))
        ));
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    } else {
        ipc::run();
        run_threads();
    }
}

fn run_threads() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let released = Arc::new(AtomicBool::new(false));
    let mut workers = Vec::new();
    for id in 1..=4 {
        let capture = CountDrop(dropped.clone());
        let released = released.clone();
        workers.push(
            thread::Builder::new()
                .stack_size(64 * 1024 + 1)
                .spawn(move || {
                    assert_eq!(INITIALIZED.get(), 0x12345678);
                    assert_eq!(ZERO.get(), 0);
                    ZERO.set(id);
                    WORKER_DROP.with(|_| ());
                    VALUES.with(|values| values.borrow_mut().push(id));
                    let local = id;
                    let mut stack = runtime::process::task()
                        .query(core::ptr::addr_of!(local) as usize)
                        .unwrap()
                        .unwrap()
                        .range;
                    while runtime::process::task()
                        .query(stack.start - 4096)
                        .unwrap()
                        .unwrap()
                        .permissions
                        .is_some()
                    {
                        stack.start -= 4096;
                    }
                    stack.end = stack.start + 68 * 1024;
                    assert!(stack.contains(&(core::ptr::addr_of!(local) as usize)));
                    while !released.load(Ordering::SeqCst) {
                        thread::yield_now().unwrap();
                    }
                    for _ in 0..4 {
                        thread::yield_now().unwrap();
                        assert_eq!(ZERO.get(), id);
                    }
                    assert_eq!(INITIALIZED.get(), 0x12345678);
                    assert!(ALIGNED.0.iter().all(|byte| *byte == 0x6b));
                    assert_eq!(core::ptr::addr_of!(ALIGNED) as usize % 8192, 0);
                    drop(capture);
                    (
                        Box::new(VALUES.with(|values| values.borrow().clone())),
                        stack,
                    )
                })
                .unwrap(),
        );
    }
    assert!(!workers[0].is_finished().unwrap());
    released.store(true, Ordering::SeqCst);
    for (index, worker) in workers.into_iter().enumerate() {
        let (values, stack) = worker.join().unwrap();
        assert_eq!(*values, vec![3, 5, 8, index + 1]);
        assert_eq!(runtime::process::task().query(stack.start).unwrap(), None);
        assert_eq!(
            runtime::process::task().query(stack.start - 4096).unwrap(),
            None
        );
    }
    assert_eq!(dropped.load(Ordering::SeqCst), 4);
    assert_eq!(TLS_DROPS.load(Ordering::SeqCst), 4);
    assert_eq!(INITIALIZED.get(), 37);
    assert_eq!(ZERO.get(), 41);

    for size in [0, usize::MAX] {
        let capture = CountDrop(dropped.clone());
        assert!(
            thread::Builder::new()
                .stack_size(size)
                .spawn(move || drop(capture))
                .is_err()
        );
    }
    assert_eq!(dropped.load(Ordering::SeqCst), 6);

    run_thread_exits();
    run_detached();
}

fn run_thread_exits() {
    let address = Arc::new(AtomicUsize::new(0));
    let child_address = address.clone();
    let canceled = thread::spawn(move || {
        let local = 53;
        child_address.store(core::ptr::addr_of!(local) as usize, Ordering::SeqCst);
        drop(child_address);
        loop {
            thread::yield_now().unwrap();
        }
    })
    .unwrap();
    wait_for(|| address.load(Ordering::SeqCst) != 0);
    canceled.thread().terminate(53).unwrap();
    assert!(matches!(
        canceled.join(),
        Err(thread::JoinError::Exited(53))
    ));
    assert!(
        runtime::process::task()
            .query(address.load(Ordering::SeqCst))
            .unwrap()
            .is_none()
    );

    let faulted = thread::Builder::new()
        .stack_size(16 * 1024)
        .spawn(|| overflow(0))
        .unwrap();
    assert!(matches!(
        faulted.join(),
        Err(thread::JoinError::Exited(usize::MAX))
    ));
    let exited = thread::spawn(|| thread::exit(71)).unwrap();
    assert!(matches!(exited.join(), Err(thread::JoinError::Exited(71))));
}

fn run_detached() {
    let released = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let mut stacks = Vec::new();
    for id in 0..20 {
        let stack = Arc::new(AtomicUsize::new(0));
        stacks.push(stack.clone());
        let released = released.clone();
        let dropped = dropped.clone();
        drop(
            thread::Builder::new()
                .stack_size(32 * 1024)
                .spawn(move || {
                    let local = id;
                    stack.store(core::ptr::addr_of!(local) as usize, Ordering::SeqCst);
                    while released.load(Ordering::SeqCst) < if id == 0 { 2 } else { 1 } {
                        thread::yield_now().unwrap();
                    }
                    CountDrop(dropped)
                })
                .unwrap(),
        );
    }
    wait_for(|| stacks.iter().all(|stack| stack.load(Ordering::SeqCst) != 0));
    released.store(1, Ordering::SeqCst);
    wait_for(|| dropped.load(Ordering::SeqCst) == 19);
    wait_for(|| {
        stacks.iter().skip(1).all(|stack| {
            let address = stack.load(Ordering::SeqCst);
            address != 0 && runtime::process::task().query(address).unwrap().is_none()
        })
    });
    assert!(
        runtime::process::task()
            .query(stacks[0].load(Ordering::SeqCst))
            .unwrap()
            .is_some()
    );
    released.store(2, Ordering::SeqCst);
    wait_for(|| dropped.load(Ordering::SeqCst) == 20);
    wait_for(|| {
        runtime::process::task()
            .query(stacks[0].load(Ordering::SeqCst))
            .unwrap()
            .is_none()
    });
}

fn wait_for(mut ready: impl FnMut() -> bool) {
    let deadline = thread::now_ns().unwrap() + 2_000_000_000;
    while !ready() {
        let now = thread::now_ns().unwrap();
        assert!(now < deadline, "thread resources were not reclaimed");
        thread::sleep_until(now + 1_000_000).unwrap();
    }
}

#[inline(never)]
fn overflow(depth: usize) -> usize {
    let block = [depth; 128];
    core::hint::black_box(&block);
    if depth == core::hint::black_box(usize::MAX) {
        depth
    } else {
        core::hint::black_box(overflow(depth + 1)) + block[0]
    }
}
