// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::state_lock;
use crate::{
    Error, Event, Handle, KernelError, Rights,
    ipc::{self, WaitItem, WaitMode},
    process,
};
use alloc::collections::VecDeque;

/// A count of available permits, with a private Event for each blocked waiter.
pub struct Semaphore {
    state: spin::Mutex<State>,
}

struct State {
    available: usize,
    waiters: VecDeque<Waiter>,
}

struct Waiter {
    key: abi::Handle,
    // Taking the signal handle commits a permit to this waiter.
    signal: Option<Handle<Event>>,
}

impl Semaphore {
    pub const fn new(available: usize) -> Self {
        Self {
            state: spin::Mutex::new(State {
                available,
                waiters: VecDeque::new(),
            }),
        }
    }

    /// Acquire one permit, leaving the count unchanged on failure.
    ///
    /// A permit committed to this waiter wins a racing timeout or wait error.
    pub fn wait(&self, mode: WaitMode) -> Result<(), Error> {
        if matches!(mode, WaitMode::Until(0 | u64::MAX)) {
            return Err(Error::Kernel(KernelError::InvalidArgument));
        }
        {
            let mut state = state_lock(&self.state);
            if state.take() {
                return Ok(());
            }
            if matches!(mode, WaitMode::Poll) {
                return Err(Error::Kernel(KernelError::WouldBlock));
            }
        }
        let event = process::task().event(Rights::WAIT | Rights::SIGNAL | Rights::DUPLICATE)?;
        let signal = event.try_clone(Rights::SIGNAL)?;
        let key = event.borrow().raw();
        {
            let mut state = state_lock(&self.state);
            if state.take() {
                return Ok(());
            }
            state
                .waiters
                .try_reserve(1)
                .map_err(|_| Error::Kernel(KernelError::NoMemory))?;
            state.waiters.push_back(Waiter {
                key,
                signal: Some(signal),
            });
        }
        let result = ipc::wait_many(&[WaitItem::new(&event, 1)], mode);
        let mut state = state_lock(&self.state);
        let index = state
            .waiters
            .iter()
            .position(|waiter| waiter.key == key)
            .unwrap();
        let waiter = state.waiters.remove(index).unwrap();
        if waiter.signal.is_none() {
            Ok(())
        } else {
            result.and(Err(Error::Protocol))
        }
    }

    /// Release one permit to the first unassigned waiter, or add it to the count.
    ///
    /// Count overflow returns `InvalidState` without changing the semaphore.
    pub fn post(&self) -> Result<(), Error> {
        let signal = {
            let mut state = state_lock(&self.state);
            if let Some(waiter) = state
                .waiters
                .iter_mut()
                .find(|waiter| waiter.signal.is_some())
            {
                waiter.signal.take()
            } else {
                state.available = state
                    .available
                    .checked_add(1)
                    .ok_or(Error::Kernel(KernelError::InvalidState))?;
                None
            }
        };
        if let Some(signal) = signal {
            signal.signal(1).expect("failed to signal semaphore waiter");
        }
        Ok(())
    }
}

impl State {
    fn take(&mut self) -> bool {
        if self.available == 0 {
            return false;
        }
        self.available -= 1;
        true
    }
}
