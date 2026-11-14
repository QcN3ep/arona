// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! futex wait indexing and wakeup registration.

use alloc::{
    collections::{BTreeMap, VecDeque},
    rc::{Rc, Weak},
};
use core::cell::{Cell, RefCell};
use runtime::{FrameAllocation, Handle};

pub(crate) type Key = (usize, usize, usize);

/// Bits of a futex word that record the owning thread ID.
pub(crate) const FUTEX_TID_MASK: u32 = 0x3fff_ffff;
/// Set when the owning thread exits without releasing the futex.
pub(crate) const FUTEX_OWNER_DIED: u32 = 0x4000_0000;
/// Set by waiters that must be woken when the owner releases the futex.
pub(crate) const FUTEX_WAITERS: u32 = 0x8000_0000;
/// Size of `struct robust_list_head` on 64-bit Linux.
pub(crate) const ROBUST_LIST_HEAD_SIZE: usize = 24;
/// Walk limit for one robust list, matching Linux `ROBUST_LIST_LIMIT`.
pub(crate) const ROBUST_LIST_LIMIT: usize = 2048;

#[derive(Default)]
pub(crate) struct Futexes {
    queues: RefCell<BTreeMap<Key, VecDeque<Weak<Waiter>>>>,
}

struct Waiter {
    key: Cell<Key>,
    mask: u32,
    ready: Cell<bool>,
    owner_died: Cell<bool>,
    frames: RefCell<Rc<Handle<FrameAllocation>>>,
}

pub(crate) struct Registration {
    table: Rc<Futexes>,
    waiter: Rc<Waiter>,
}

impl Futexes {
    pub fn register(
        self: &Rc<Self>,
        key: Key,
        mask: u32,
        frames: Rc<Handle<FrameAllocation>>,
    ) -> Registration {
        let waiter = Rc::new(Waiter {
            key: Cell::new(key),
            mask,
            ready: Cell::new(false),
            owner_died: Cell::new(false),
            frames: RefCell::new(frames),
        });
        self.queues
            .borrow_mut()
            .entry(key)
            .or_default()
            .push_back(Rc::downgrade(&waiter));
        Registration {
            table: Rc::clone(self),
            waiter,
        }
    }

    pub fn wake(&self, key: Key, count: usize, mask: u32) -> usize {
        let mut queues = self.queues.borrow_mut();
        let Some(queue) = queues.get_mut(&key) else {
            return 0;
        };
        let mut woken = 0;
        queue.retain(|waiter| {
            let Some(waiter) = waiter.upgrade() else {
                return false;
            };
            if woken < count && waiter.mask & mask != 0 {
                waiter.ready.set(true);
                woken += 1;
                false
            } else {
                true
            }
        });
        if queue.is_empty() {
            queues.remove(&key);
        }
        woken
    }

    /// Wake one waiter because the futex owner died.
    ///
    /// The woken waiter is told why so it can report `EOWNERDEAD`, and the
    /// caller has already marked the futex word. Linux wakes at most one
    /// waiter here as well; the next owner death or unlock wakes the next one.
    pub fn wake_owner_died(&self, key: Key) -> usize {
        let mut queues = self.queues.borrow_mut();
        let Some(queue) = queues.get_mut(&key) else {
            return 0;
        };
        let mut woken = false;
        queue.retain(|entry| {
            let Some(waiter) = entry.upgrade() else {
                return false;
            };
            if woken {
                return true;
            }
            waiter.owner_died.set(true);
            waiter.ready.set(true);
            woken = true;
            false
        });
        if queue.is_empty() {
            queues.remove(&key);
        }
        usize::from(woken)
    }

    pub fn requeue(
        &self,
        source: Key,
        target: Key,
        frames: Rc<Handle<FrameAllocation>>,
        count: usize,
    ) -> usize {
        if source == target {
            return 0;
        }
        let mut queues = self.queues.borrow_mut();
        let Some(mut queue) = queues.remove(&source) else {
            return 0;
        };
        let mut moved = 0;
        let target_queue = queues.entry(target).or_default();
        while moved < count {
            let Some(entry) = queue.pop_front() else {
                break;
            };
            if let Some(waiter) = entry.upgrade() {
                waiter.key.set(target);
                *waiter.frames.borrow_mut() = Rc::clone(&frames);
                target_queue.push_back(entry);
                moved += 1;
            }
        }
        if target_queue.is_empty() {
            queues.remove(&target);
        }
        if !queue.is_empty() {
            queues.insert(source, queue);
        }
        moved
    }
}

impl Registration {
    pub fn ready(&self) -> bool {
        self.waiter.ready.get()
    }

    /// Report whether the futex owner died before this waiter was woken.
    pub fn owner_died(&self) -> bool {
        self.waiter.owner_died.get()
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        let mut queues = self.table.queues.borrow_mut();
        let key = self.waiter.key.get();
        if let Some(queue) = queues.get_mut(&key) {
            queue.retain(|waiter| {
                !waiter.ptr_eq(&Rc::downgrade(&self.waiter)) && waiter.strong_count() != 0
            });
            if queue.is_empty() {
                queues.remove(&key);
            }
        }
    }
}
