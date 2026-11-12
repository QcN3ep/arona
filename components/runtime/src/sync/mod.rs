// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Synchronization between threads in the current native program.
//!
//! Mutexes, counting semaphores, and bounded mailboxes sleep through private
//! Events when unavailable. Waits use the same polling and absolute deadlines as
//! IPC. Terminating a thread without running its destructors does not recover
//! its locks or complete its in-progress operations.

mod mailbox;
mod mutex;
mod semaphore;

pub use mailbox::Mailbox;
pub use mutex::{Mutex, MutexGuard};
pub use semaphore::Semaphore;

use crate::thread;

fn state_lock<T>(state: &spin::Mutex<T>) -> spin::MutexGuard<'_, T> {
    loop {
        if let Some(guard) = state.try_lock() {
            return guard;
        }
        // A preempted holder must be able to run on a single CPU.
        thread::yield_now().expect("failed to yield for synchronization state");
    }
}
