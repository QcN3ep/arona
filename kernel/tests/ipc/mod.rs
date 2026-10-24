// SPDX-License-Identifier: MPL-2.0

mod channel;
mod event;
mod timer;
mod wait;

pub fn run() {
    let manager = kernel::memory::manager();
    manager.reclaim_heap();
    let heap = manager.heap_stats();
    let frames = manager.stats();
    channel::run();
    event::run();
    timer::run();
    wait::run();
    manager.reclaim_heap();
    assert_eq!(manager.heap_stats(), heap);
    assert_eq!(manager.stats(), frames);
}

fn run_thread(entry: kernel::thread::ThreadEntry) {
    use kernel::{
        hardware,
        thread::{Thread, ThreadState, scheduler},
    };
    let owner = Thread::new(entry, 0).unwrap();
    owner.start().unwrap();
    let limit = hardware::timer().now_ns().unwrap() + 2_000_000_000;
    loop {
        scheduler::run_ready().unwrap();
        if owner.state() == ThreadState::Dead(0) {
            break;
        }
        assert!(
            hardware::timer().now_ns().unwrap() < limit,
            "IPC wait stalled"
        );
        hardware::cpu().idle_until_interrupt();
    }
}
