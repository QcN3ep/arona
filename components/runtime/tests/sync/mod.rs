// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

mod mailbox;
mod mutex;
mod semaphore;

use runtime::thread::{JoinHandle, ThreadState};

pub fn run() {
    semaphore::run();
    mutex::run();
    mailbox::run();
}

fn blocked<T: Send + 'static>(worker: &JoinHandle<T>) {
    super::wait_for(|| worker.thread().state().unwrap().0 == ThreadState::Blocked);
}
