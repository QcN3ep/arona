// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Non-consuming socket snapshots and independent, coalescing wake events.

use crate::{
    Error,
    teardown::{Shared, lock},
};
use alloc::{sync::Arc, vec::Vec};
use core::{
    ffi::c_void,
    sync::atomic::{AtomicU64, Ordering},
};
use runtime::{
    Event, Handle, Rights,
    ipc::{self, WaitItem, WaitMode},
    process,
};

const READABLE: u64 = 1;
const WRITABLE: u64 = 2;
const ERROR: u64 = 4;
const READ_CLOSED: u64 = 8;
const WRITE_CLOSED: u64 = 16;
const CLOSED: u64 = 32;
const FLAGS: u64 = 255;

/// A readiness hint. An I/O attempt can still return `WouldBlock`.
///
/// A writable connecting socket requires `Socket::take_error` before use.
/// `error` records a connection failure and does not consume `SO_ERROR`.
/// `read_closed` becomes sticky after receiving EOF or shutting down reads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Readiness {
    /// A wrapping 56-bit sequence, meaningful only within this socket's lifetime.
    pub sequence: u64,
    pub readable: bool,
    pub writable: bool,
    pub error: bool,
    pub read_closed: bool,
    pub write_closed: bool,
    pub closed: bool,
}

pub(super) struct Notices {
    snapshot: AtomicU64,
    subscribers: spin::Mutex<Vec<Arc<Handle<Event>>>>,
}

impl Notices {
    pub(super) fn new() -> Self {
        Self {
            snapshot: AtomicU64::new(0),
            subscribers: spin::Mutex::new(Vec::new()),
        }
    }

    pub(super) fn snapshot(&self) -> Readiness {
        let value = self.snapshot.load(Ordering::Acquire);
        Readiness {
            sequence: value >> 8,
            readable: value & READABLE != 0,
            writable: value & WRITABLE != 0,
            error: value & ERROR != 0,
            read_closed: value & READ_CLOSED != 0,
            write_closed: value & WRITE_CLOSED != 0,
            closed: value & CLOSED != 0,
        }
    }

    // Publication under C protection only touches an atomic. Signaling runs
    // after that lock is released, while C still retains its entry reference.
    pub(super) fn publish(&self, flags: u64) -> bool {
        let mut previous = self.snapshot.load(Ordering::Acquire);
        loop {
            if previous & CLOSED != 0 || previous & FLAGS == flags {
                return false;
            }
            let next = (previous & !FLAGS).wrapping_add(256) | flags;
            match self.snapshot.compare_exchange_weak(
                previous,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(value) => previous = value,
            }
        }
    }

    pub(super) fn notify(&self) {
        // Registration and removal share this short lock with signaling. No
        // callback allocates, duplicates handles, or waits for protocol work.
        for event in lock(&self.subscribers).iter() {
            event.signal(1).expect("failed to signal socket subscriber");
        }
    }

    pub(super) fn close(&self) {
        let error = self.snapshot.load(Ordering::Acquire) & ERROR;
        self.publish(CLOSED | READABLE | READ_CLOSED | WRITE_CLOSED | error);
        self.notify();
    }
}

/// An independent waiter tied to one socket's lifetime, including after close.
///
/// Subscribe before checking readiness or attempting I/O. Pass the snapshot
/// taken before that attempt to `wait` after `WouldBlock`. Changes that race
/// registration or sleeping either change the sequence or leave an Event bit.
/// Recheck readiness after waking. Notifications may coalesce or be stale.
pub struct Subscription {
    shared: Arc<Shared>,
    event: Arc<Handle<Event>>,
}

impl Subscription {
    pub(super) fn new(shared: Arc<Shared>) -> Result<Self, Error> {
        let event = process::task().event(Rights::WAIT | Rights::SIGNAL)?;
        let event = Arc::try_new(event).map_err(|_| Error::NoMemory)?;
        {
            let mut subscribers = lock(&shared.notices.subscribers);
            subscribers.try_reserve(1).map_err(|_| Error::NoMemory)?;
            subscribers.push(Arc::clone(&event));
        }
        Ok(Self { shared, event })
    }

    pub fn snapshot(&self) -> Readiness {
        self.shared.notices.snapshot()
    }

    /// The wake capability for integration with `runtime::ipc::wait_many`.
    /// Use one waiting consumer per subscription, then recheck the snapshot.
    pub fn event(&self) -> &Handle<Event> {
        &self.event
    }

    /// Wait for a sequence change without consuming socket data or errors.
    /// `observed` must have come from this subscription or its socket.
    pub fn wait(&mut self, observed: Readiness, mode: WaitMode) -> Result<Readiness, Error> {
        loop {
            let snapshot = self.snapshot();
            if snapshot.sequence != observed.sequence {
                return Ok(snapshot);
            }
            ipc::wait_many(&[WaitItem::new(self.event(), 1)], mode)?;
        }
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let removed = {
            let mut subscribers = lock(&self.shared.notices.subscribers);
            let index = subscribers
                .iter()
                .position(|event| Arc::ptr_eq(event, &self.event))
                .unwrap();
            subscribers.swap_remove(index)
        };
        drop(removed);
    }
}

/// # Safety
/// context must be the live Arc retained by a C socket entry. C serializes
/// publications under its protection lock and supplies only defined flag bits.
#[unsafe(no_mangle)]
unsafe extern "C" fn arona_lwip_publish(context: *const c_void, flags: u32) -> i32 {
    // SAFETY: The entry retains the Arc for the entire callback.
    let shared = unsafe { &*context.cast::<Shared>() };
    i32::from(shared.notices.publish(flags.into()))
}

/// # Safety
/// context must still be retained by the entry, outside C protection.
#[unsafe(no_mangle)]
unsafe extern "C" fn arona_lwip_notify(context: *const c_void) {
    // SAFETY: The entry retains the Arc for the entire callback.
    unsafe { &*context.cast::<Shared>() }.notices.notify();
}

/// # Safety
/// context transfers the Arc reference attached to an entry. Call exactly once
/// after the final C API/callback reference leaves and outside C protection.
#[unsafe(no_mangle)]
unsafe extern "C" fn arona_lwip_release(context: *const c_void) {
    // SAFETY: Final entry destruction transfers its one raw Arc reference.
    drop(unsafe { Arc::from_raw(context.cast::<Shared>()) });
}
