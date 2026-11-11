// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use arrayvec::ArrayVec;

use super::{MAX_THREADS, Thread};

pub(super) struct DeadlineQueue {
    entries: ArrayVec<Deadline, MAX_THREADS>,
}

struct Deadline {
    thread: Thread,
    at: u64,
}

impl DeadlineQueue {
    pub const fn new() -> Self {
        Self {
            entries: ArrayVec::new_const(),
        }
    }

    pub fn insert(&mut self, thread: Thread, at: u64) {
        let index = self.entries.len();
        self.entries.push(Deadline { thread, at });
        self.set_index(index);
        self.sift_up(index);
    }

    pub fn remove(&mut self, index: usize) -> Thread {
        let removed = self.entries.swap_remove(index);
        removed
            .thread
            .0
            .data
            .lock()
            .wait
            .as_mut()
            .unwrap()
            .deadline_index = None;
        if index < self.entries.len() {
            self.set_index(index);
            if index > 0 && self.entries[index].at < self.entries[(index - 1) / 2].at {
                self.sift_up(index);
            } else {
                self.sift_down(index);
            }
        }
        removed.thread
    }

    pub fn pop_due(&mut self, now: u64) -> Option<Thread> {
        if self.entries.first().is_some_and(|entry| entry.at <= now) {
            Some(self.remove(0))
        } else {
            None
        }
    }

    pub fn next_deadline(&self) -> Option<u64> {
        self.entries.first().map(|entry| entry.at)
    }

    fn sift_up(&mut self, mut index: usize) {
        while index > 0 {
            let parent = (index - 1) / 2;
            if self.entries[parent].at <= self.entries[index].at {
                break;
            }
            self.swap(parent, index);
            index = parent;
        }
    }

    fn sift_down(&mut self, mut index: usize) {
        loop {
            let left = index * 2 + 1;
            if left >= self.entries.len() {
                break;
            }
            let right = left + 1;
            let child =
                if right < self.entries.len() && self.entries[right].at < self.entries[left].at {
                    right
                } else {
                    left
                };
            if self.entries[index].at <= self.entries[child].at {
                break;
            }
            self.swap(index, child);
            index = child;
        }
    }

    fn swap(&mut self, first: usize, second: usize) {
        self.entries.swap(first, second);
        self.set_index(first);
        self.set_index(second);
    }

    fn set_index(&self, index: usize) {
        let mut data = self.entries[index].thread.0.data.lock();
        data.wait.as_mut().unwrap().deadline_index = Some(index);
    }
}
