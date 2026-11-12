// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::{Semaphore, state_lock};
use crate::{Error, KernelError, ipc::WaitMode};
use alloc::collections::VecDeque;

/// A bounded FIFO whose blocked senders retain ownership until space is acquired.
pub struct Mailbox<T> {
    queue: spin::Mutex<VecDeque<T>>,
    writable: Semaphore,
    readable: Semaphore,
}

impl<T> Mailbox<T> {
    /// Allocate storage for a nonzero number of queued values.
    pub fn new(capacity: usize) -> Result<Self, Error> {
        if capacity == 0 {
            return Err(Error::Kernel(KernelError::InvalidArgument));
        }
        let mut queue = VecDeque::new();
        queue
            .try_reserve_exact(capacity)
            .map_err(|_| Error::Kernel(KernelError::NoMemory))?;
        Ok(Self {
            queue: spin::Mutex::new(queue),
            writable: Semaphore::new(capacity),
            readable: Semaphore::new(0),
        })
    }

    /// Enqueue a value; failure returns both the error and the unconsumed value.
    pub fn send(&self, value: T, mode: WaitMode) -> Result<(), (Error, T)> {
        if let Err(error) = self.writable.wait(mode) {
            return Err((error, value));
        }
        state_lock(&self.queue).push_back(value);
        self.readable
            .post()
            .expect("failed to publish mailbox item");
        Ok(())
    }

    pub fn receive(&self, mode: WaitMode) -> Result<T, Error> {
        self.readable.wait(mode)?;
        let value = state_lock(&self.queue).pop_front().unwrap();
        self.writable
            .post()
            .expect("failed to release mailbox slot");
        Ok(value)
    }
}
