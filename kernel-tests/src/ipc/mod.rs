// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

mod call;
mod channel;
mod event;
mod reply;
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
    reply::run();
    call::run();
    manager.reclaim_heap();
    assert_eq!(manager.heap_stats(), heap);
    assert_eq!(manager.stats(), frames);
}

use crate::run_thread;
