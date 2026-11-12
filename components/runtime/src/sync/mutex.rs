// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::Semaphore;
use crate::{Error, ipc::WaitMode};
use core::{
    cell::UnsafeCell,
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

/// Exclusive access to a value; recursive acquisition waits like any contention.
pub struct Mutex<T> {
    available: Semaphore,
    value: UnsafeCell<T>,
}

/// Exclusive access that releases the mutex when dropped, without poisoning.
#[must_use]
pub struct MutexGuard<'a, T> {
    mutex: &'a Mutex<T>,
    // Sharing the guard also requires T: Sync, unlike sharing the mutex itself.
    _exclusive: PhantomData<&'a mut T>,
}

// SAFETY: Access to the value is exclusive to the holder of the semaphore permit.
// Moving that ownership between threads requires T: Send.
unsafe impl<T: Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self {
        Self {
            available: Semaphore::new(1),
            value: UnsafeCell::new(value),
        }
    }

    pub fn lock(&self, mode: WaitMode) -> Result<MutexGuard<'_, T>, Error> {
        self.available.wait(mode)?;
        Ok(MutexGuard {
            mutex: self,
            _exclusive: PhantomData,
        })
    }
}

impl<T> Deref for MutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: This guard holds the only permit for access to the value.
        unsafe { &*self.mutex.value.get() }
    }
}

impl<T> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: The guard holds the only permit and is exclusively borrowed.
        unsafe { &mut *self.mutex.value.get() }
    }
}

impl<T> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        self.mutex
            .available
            .post()
            .expect("failed to release mutex");
    }
}
