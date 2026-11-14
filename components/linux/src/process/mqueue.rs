// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! POSIX message queue objects, limits, and notification registration.

use alloc::{
    collections::{BTreeMap, VecDeque},
    rc::{Rc, Weak},
    vec::Vec,
};
use core::cell::{Cell, RefCell};

pub(crate) const NAME_MAX: usize = 255;
pub(crate) const DEFAULT_MAXMSG: usize = 10;
pub(crate) const DEFAULT_MSGSIZE: usize = 8192;
pub(crate) const MAXMSG_LIMIT: usize = 1024;
pub(crate) const MSGSIZE_LIMIT: usize = 65536;
pub(crate) const PRIORITY_MAX: u32 = 32768;
pub(crate) const DIRECTORY_INODE: u64 = 0x6d71_0001;

pub(crate) struct Queues {
    entries: RefCell<BTreeMap<Vec<u8>, Rc<Queue>>>,
    notifications: RefCell<BTreeMap<usize, Vec<Weak<Queue>>>>,
    next_inode: Cell<u64>,
}

impl Queues {
    pub fn new() -> Self {
        Self {
            entries: RefCell::new(BTreeMap::new()),
            notifications: RefCell::new(BTreeMap::new()),
            next_inode: Cell::new(0x6d71_0010),
        }
    }

    pub fn next_inode(&self) -> u64 {
        let inode = self.next_inode.get();
        self.next_inode.set(inode.saturating_add(1));
        inode
    }

    pub fn get(&self, name: &[u8]) -> Option<Rc<Queue>> {
        self.entries.borrow().get(name).cloned()
    }

    pub fn insert(&self, name: Vec<u8>, queue: Rc<Queue>) -> bool {
        let mut entries = self.entries.borrow_mut();
        if entries.contains_key(&name) {
            return false;
        }
        entries.insert(name, queue);
        true
    }

    pub fn remove(&self, name: &[u8]) -> Option<Rc<Queue>> {
        self.entries.borrow_mut().remove(name)
    }

    pub fn list(&self) -> Vec<Rc<Queue>> {
        self.entries.borrow().values().cloned().collect()
    }

    /// Record a queue that may hold a notification for a process, including unlinked queues.
    pub fn track_notification(&self, pid: usize, queue: &Rc<Queue>) {
        let mut notifications = self.notifications.borrow_mut();
        let entries = notifications.entry(pid).or_default();
        entries.retain(|entry| entry.strong_count() != 0);
        entries.push(Rc::downgrade(queue));
    }

    /// Drop notification registrations owned by an exiting process.
    pub fn release(&self, pid: usize) {
        if let Some(entries) = self.notifications.borrow_mut().remove(&pid) {
            for entry in entries {
                if let Some(queue) = entry.upgrade() {
                    queue.clear_notify(pid);
                }
            }
        }
        for queue in self.entries.borrow().values() {
            queue.clear_notify(pid);
        }
    }
}

pub(crate) struct Queue {
    name: Vec<u8>,
    inode: u64,
    maxmsg: usize,
    msgsize: usize,
    uid: u32,
    gid: u32,
    mode: u32,
    messages: RefCell<VecDeque<Message>>,
    notify: RefCell<Option<Notify>>,
    receivers: Cell<usize>,
}

pub(crate) struct Message {
    pub data: Vec<u8>,
    pub priority: u32,
}

#[derive(Clone, Copy)]
pub(crate) struct Notify {
    pub pid: usize,
    pub value: u64,
    pub signo: i32,
    pub notify: i32,
}

impl Queue {
    pub fn new(
        name: Vec<u8>,
        inode: u64,
        maxmsg: usize,
        msgsize: usize,
        uid: u32,
        gid: u32,
        mode: u32,
    ) -> Self {
        Self {
            name,
            inode,
            maxmsg,
            msgsize,
            uid,
            gid,
            mode,
            messages: RefCell::new(VecDeque::new()),
            notify: RefCell::new(None),
            receivers: Cell::new(0),
        }
    }

    pub fn name(&self) -> &[u8] {
        &self.name
    }

    pub fn inode(&self) -> u64 {
        self.inode
    }

    pub fn maxmsg(&self) -> usize {
        self.maxmsg
    }

    pub fn msgsize(&self) -> usize {
        self.msgsize
    }

    pub fn uid(&self) -> u32 {
        self.uid
    }

    pub fn gid(&self) -> u32 {
        self.gid
    }

    pub fn mode(&self) -> u32 {
        self.mode
    }

    pub fn current(&self) -> usize {
        self.messages.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.borrow().is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.messages.borrow().len() >= self.maxmsg
    }

    /// Insert a message in descending priority order, preserving FIFO order per priority.
    pub fn push(&self, priority: u32, data: Vec<u8>) -> Result<(), ()> {
        let mut messages = self.messages.borrow_mut();
        if messages.len() >= self.maxmsg {
            return Err(());
        }
        messages.try_reserve(1).map_err(|_| ())?;
        let index = messages
            .iter()
            .position(|message| message.priority < priority)
            .unwrap_or(messages.len());
        messages.insert(index, Message { data, priority });
        Ok(())
    }

    pub fn pop(&self) -> Option<Message> {
        self.messages.borrow_mut().pop_front()
    }

    /// Register a one-shot notification, rejecting a second registration.
    pub fn register_notify(&self, notification: Notify) -> bool {
        let mut notify = self.notify.borrow_mut();
        if notify.is_some() {
            return false;
        }
        *notify = Some(notification);
        true
    }

    pub fn take_notify(&self) -> Option<Notify> {
        self.notify.borrow_mut().take()
    }

    pub fn clear_notify(&self, pid: usize) {
        let mut notify = self.notify.borrow_mut();
        if notify.as_ref().is_some_and(|notify| notify.pid == pid) {
            *notify = None;
        }
    }

    pub fn notify(&self) -> Option<Notify> {
        *self.notify.borrow()
    }

    pub fn receivers(&self) -> usize {
        self.receivers.get()
    }

    pub fn receive_guard(self: &Rc<Self>) -> Receiver {
        self.receivers.set(self.receivers.get().saturating_add(1));
        Receiver {
            queue: Rc::clone(self),
        }
    }
}

/// Tracks a thread blocked in `mq_timedreceive` so senders can suppress notifications.
pub(crate) struct Receiver {
    queue: Rc<Queue>,
}

impl Drop for Receiver {
    fn drop(&mut self) {
        self.queue
            .receivers
            .set(self.queue.receivers.get().saturating_sub(1));
    }
}
