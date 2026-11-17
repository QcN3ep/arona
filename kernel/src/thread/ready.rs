// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Allocation-free red-black tree of runnable threads, ordered by virtual runtime.

use super::Thread;

#[derive(Default)]
pub(super) struct Links {
    children: [Option<Thread>; 2],
    red: bool,
    queued: bool,
    sequence: u64,
}

pub(super) struct ReadyQueue {
    root: Option<Thread>,
    first: Option<Thread>,
    pub count: usize,
    pub weight: u64,
    sequence: u64,
}

impl ReadyQueue {
    pub const fn new() -> Self {
        Self {
            root: None,
            first: None,
            count: 0,
            weight: 0,
            sequence: 0,
        }
    }

    pub fn first(&self) -> Option<&Thread> {
        self.first.as_ref()
    }

    pub fn push(&mut self, thread: Thread) {
        {
            let mut data = thread.0.data.lock();
            data.ready.sequence = self.sequence;
            self.sequence = self
                .sequence
                .checked_add(1)
                .expect("ready sequence exhausted");
        }
        self.restore(thread);
    }

    pub fn restore(&mut self, thread: Thread) {
        let key = {
            let mut data = thread.0.data.lock();
            assert!(!data.ready.queued);
            data.ready.queued = true;
            data.ready.red = true;
            self.weight += u64::from(data.fair.weight());
            (data.fair.vruntime, data.ready.sequence)
        };
        if self
            .first
            .as_ref()
            .is_none_or(|first| key < node_key(first))
        {
            self.first = Some(thread.clone());
        }
        self.root = Some(insert(self.root.take(), thread, key));
        set_red(self.root.as_ref().unwrap(), false);
        self.count += 1;
    }

    pub fn remove(&mut self, thread: &Thread) {
        let key = node_key(thread);
        assert!(thread.0.data.lock().ready.queued);
        let root = self.root.take().expect("missing ready root");
        if !is_red(&child(&root, 0)) && !is_red(&child(&root, 1)) {
            set_red(&root, true);
        }
        self.root = remove(root, key);
        if let Some(root) = &self.root {
            set_red(root, false);
        }
        self.first = self.root.clone().map(minimum);
        self.count -= 1;
        let mut data = thread.0.data.lock();
        self.weight -= u64::from(data.fair.weight());
        data.ready.children = [None, None];
        data.ready.queued = false;
        data.ready.red = false;
    }

    pub fn pop(&mut self) -> Option<Thread> {
        let first = self.first.clone()?;
        self.remove(&first);
        Some(first)
    }
}

fn node_key(thread: &Thread) -> (i128, u64) {
    let data = thread.0.data.lock();
    (data.fair.vruntime, data.ready.sequence)
}

fn child(thread: &Thread, direction: usize) -> Option<Thread> {
    thread.0.data.lock().ready.children[direction].clone()
}

fn take_child(thread: &Thread, direction: usize) -> Option<Thread> {
    thread.0.data.lock().ready.children[direction].take()
}

fn set_child(thread: &Thread, direction: usize, child: Option<Thread>) {
    thread.0.data.lock().ready.children[direction] = child;
}

fn is_red(thread: &Option<Thread>) -> bool {
    thread
        .as_ref()
        .is_some_and(|thread| thread.0.data.lock().ready.red)
}

fn set_red(thread: &Thread, red: bool) {
    thread.0.data.lock().ready.red = red;
}

fn minimum(mut thread: Thread) -> Thread {
    while let Some(left) = child(&thread, 0) {
        thread = left;
    }
    thread
}

// REFERENCE: https://www.cs.princeton.edu/~rs/talks/LLRB/LLRB.pdf
// Child links own their subtrees. Rotations transfer ownership without parent links or cycles.
fn rotate(root: Thread, direction: usize) -> Thread {
    let next = take_child(&root, 1 - direction).unwrap();
    set_child(&root, 1 - direction, take_child(&next, direction));
    let red = root.0.data.lock().ready.red;
    set_red(&next, red);
    set_red(&root, true);
    set_child(&next, direction, Some(root));
    next
}

fn flip(root: &Thread) {
    {
        let mut data = root.0.data.lock();
        data.ready.red = !data.ready.red;
    }
    for direction in 0..2 {
        let node = child(root, direction).unwrap();
        let mut data = node.0.data.lock();
        data.ready.red = !data.ready.red;
    }
}

fn balance(mut root: Thread) -> Thread {
    if is_red(&child(&root, 1)) {
        root = rotate(root, 0);
    }
    if is_red(&child(&root, 0)) && is_red(&child(&child(&root, 0).unwrap(), 0)) {
        root = rotate(root, 1);
    }
    if is_red(&child(&root, 0)) && is_red(&child(&root, 1)) {
        flip(&root);
    }
    root
}

fn insert(root: Option<Thread>, thread: Thread, key: (i128, u64)) -> Thread {
    let Some(root) = root else { return thread };
    let direction = usize::from(key >= node_key(&root));
    let subtree = insert(take_child(&root, direction), thread, key);
    set_child(&root, direction, Some(subtree));
    balance(root)
}

fn move_red_left(mut root: Thread) -> Thread {
    flip(&root);
    if is_red(&child(&child(&root, 1).unwrap(), 0)) {
        let right = rotate(take_child(&root, 1).unwrap(), 1);
        set_child(&root, 1, Some(right));
        root = rotate(root, 0);
        flip(&root);
    }
    root
}

fn move_red_right(mut root: Thread) -> Thread {
    flip(&root);
    if is_red(&child(&child(&root, 0).unwrap(), 0)) {
        root = rotate(root, 1);
        flip(&root);
    }
    root
}

fn take_minimum(mut root: Thread) -> (Thread, Option<Thread>) {
    let Some(left) = child(&root, 0) else {
        return (root, None);
    };
    if !is_red(&Some(left.clone())) && !is_red(&child(&left, 0)) {
        root = move_red_left(root);
    }
    let (first, remaining) = take_minimum(take_child(&root, 0).unwrap());
    set_child(&root, 0, remaining);
    (first, Some(balance(root)))
}

fn remove(mut root: Thread, key: (i128, u64)) -> Option<Thread> {
    if key < node_key(&root) {
        let left = child(&root, 0).expect("missing ready thread");
        if !is_red(&Some(left.clone())) && !is_red(&child(&left, 0)) {
            root = move_red_left(root);
        }
        let remaining = remove(take_child(&root, 0).unwrap(), key);
        set_child(&root, 0, remaining);
    } else {
        if is_red(&child(&root, 0)) {
            root = rotate(root, 1);
        }
        if key == node_key(&root) && child(&root, 1).is_none() {
            return None;
        }
        let right = child(&root, 1).expect("missing ready thread");
        if !is_red(&Some(right.clone())) && !is_red(&child(&right, 0)) {
            root = move_red_right(root);
        }
        if key == node_key(&root) {
            let (next, remaining) = take_minimum(take_child(&root, 1).unwrap());
            set_child(&next, 0, take_child(&root, 0));
            set_child(&next, 1, remaining);
            let red = root.0.data.lock().ready.red;
            set_red(&next, red);
            root = next;
        } else {
            let remaining = remove(take_child(&root, 1).unwrap(), key);
            set_child(&root, 1, remaining);
        }
    }
    Some(balance(root))
}
