// SPDX-License-Identifier: MPL-2.0

use core::ops::Deref;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

use super::wait::{Observer, Observers, notify_observers};

use crate::{
    hardware::cpu::InterruptGuard,
    memory::{self, HeapArc, MemoryError},
};

/// # Safety
///
/// Operations and drop must retain the kernel mappings required by the memory manager.
pub struct Event {
    reference: EventReference,
}

/// Keeps the object storage alive; not counted as an open reference.
///
/// # Safety
///
/// Operations and drop must retain the kernel mappings required by the memory manager.
#[derive(Clone)]
pub struct EventReference {
    event: HeapArc<Mutex<EventState>>,
}

struct EventState {
    credits: usize,
    pending: u64,
    closed: bool,
    observers: Observers,
}

static EVENTS: AtomicUsize = AtomicUsize::new(0);

pub fn active_events() -> usize {
    EVENTS.load(Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventError {
    InvalidArgument,
    Closed,
    Memory(MemoryError),
}

impl Event {
    pub fn new() -> Result<Self, EventError> {
        let _interrupts = InterruptGuard::new();
        let event = memory::manager().allocate_arc(Mutex::new(EventState {
            credits: 1,
            pending: 0,
            closed: false,
            observers: Observers::new(),
        }))?;
        EVENTS.fetch_add(1, Ordering::Relaxed);
        Ok(Self {
            reference: EventReference { event },
        })
    }

    pub fn reference(&self) -> EventReference {
        self.reference.clone()
    }
}

impl Drop for EventState {
    fn drop(&mut self) {
        EVENTS.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Clone for Event {
    fn clone(&self) -> Self {
        let _interrupts = InterruptGuard::new();
        self.reference.event.lock().credits += 1;
        Self {
            reference: self.reference(),
        }
    }
}

impl Deref for Event {
    type Target = EventReference;

    fn deref(&self) -> &Self::Target {
        &self.reference
    }
}

impl EventReference {
    pub fn signal(&self, mask: u64) -> Result<(), EventError> {
        if mask == 0 {
            return Err(EventError::InvalidArgument);
        }
        let _interrupts = InterruptGuard::new();
        let mut state = self.event.lock();
        if state.closed {
            return Err(EventError::Closed);
        }
        state.pending |= mask;
        drop(state);
        self.notify_observers();
        Ok(())
    }

    pub(crate) fn readiness(&self, mask: u64) -> Result<u64, EventError> {
        if mask == 0 {
            return Err(EventError::InvalidArgument);
        }
        let _interrupts = InterruptGuard::new();
        let state = self.event.lock();
        if state.closed {
            return Err(EventError::Closed);
        }
        Ok(state.pending & mask)
    }

    pub(crate) fn consume(&self, mask: u64) -> Result<u64, EventError> {
        let _interrupts = InterruptGuard::new();
        let mut state = self.event.lock();
        if state.closed {
            return Err(EventError::Closed);
        }
        let delivered = state.pending & mask;
        state.pending &= !delivered;
        Ok(delivered)
    }

    pub(crate) fn register_observer(&self, observer: Observer) -> Result<(), MemoryError> {
        self.event.lock().observers.insert(observer)
    }

    pub(crate) fn unregister_observer(&self, observer: &Observer) {
        self.event.lock().observers.remove(observer);
    }

    fn notify_observers(&self) {
        notify_observers(|index| self.event.lock().observers.get(index));
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.event.lock().closed
    }

    pub(crate) fn same(&self, other: &Self) -> bool {
        HeapArc::ptr_eq(&self.event, &other.event)
    }

    pub(crate) fn identity(&self) -> usize {
        self.event.as_ptr().addr()
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        let _interrupts = InterruptGuard::new();
        let mut state = self.reference.event.lock();
        state.credits -= 1;
        if state.credits == 0 {
            state.closed = true;
            state.pending = 0;
            drop(state);
            super::timer::cancel_event(&self.reference);
            crate::device::close_event(&self.reference);
            self.reference.notify_observers();
        }
    }
}

impl From<MemoryError> for EventError {
    fn from(error: MemoryError) -> Self {
        Self::Memory(error)
    }
}
