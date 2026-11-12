// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Pipe byte queues and endpoint lifetime.

use alloc::{collections::VecDeque, rc::Rc, vec::Vec};
use core::{
    cell::RefCell,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_PIPE: AtomicU64 = AtomicU64::new(1);

pub(crate) const PIPE_BUF: usize = 4096;
pub(crate) const DEFAULT_CAPACITY: usize = 16 * PIPE_BUF;
pub(crate) const MAX_CAPACITY: usize = 1024 * 1024;
const PAGE_SIZE: usize = 4096;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum CapacityError {
    TooLarge,
    Busy,
    NoMemory,
}

struct Pipe {
    id: u64,
    bytes: VecDeque<u8>,
    capacity: usize,
    reader: bool,
    writer: bool,
}

pub(crate) struct PipeEnd {
    pipe: Rc<RefCell<Pipe>>,
    pub read: bool,
}

impl PipeEnd {
    pub fn pair() -> Result<(Self, Self), ()> {
        let mut bytes = VecDeque::new();
        bytes.try_reserve_exact(DEFAULT_CAPACITY).map_err(|_| ())?;
        let mut id = NEXT_PIPE.load(Ordering::Relaxed);
        loop {
            let next = id.checked_add(1).ok_or(())?;
            match NEXT_PIPE.compare_exchange_weak(id, next, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => break,
                Err(current) => id = current,
            }
        }
        let pipe = Rc::new(RefCell::new(Pipe {
            id,
            bytes,
            capacity: DEFAULT_CAPACITY,
            reader: true,
            writer: true,
        }));
        Ok((
            Self {
                pipe: Rc::clone(&pipe),
                read: true,
            },
            Self { pipe, read: false },
        ))
    }

    pub fn inode(&self) -> u64 {
        self.pipe.borrow().id
    }

    pub fn snapshot(&self, count: usize) -> Vec<u8> {
        self.pipe
            .borrow()
            .bytes
            .iter()
            .copied()
            .take(count)
            .collect()
    }

    pub fn consume(&self, count: usize) {
        self.pipe.borrow_mut().bytes.drain(..count);
    }

    pub fn available(&self) -> usize {
        self.pipe.borrow().bytes.len()
    }

    pub fn capacity(&self) -> usize {
        self.pipe.borrow().capacity
    }

    pub fn set_capacity(&self, requested: usize) -> Result<usize, CapacityError> {
        let pages = requested.max(PAGE_SIZE).div_ceil(PAGE_SIZE);
        let capacity = pages
            .checked_next_power_of_two()
            .and_then(|pages| pages.checked_mul(PAGE_SIZE))
            .filter(|capacity| *capacity <= MAX_CAPACITY)
            .ok_or(CapacityError::TooLarge)?;
        let mut pipe = self.pipe.borrow_mut();
        if capacity < pipe.bytes.len() {
            return Err(CapacityError::Busy);
        }
        let length = pipe.bytes.len();
        if capacity > pipe.bytes.capacity() {
            pipe.bytes
                .try_reserve(capacity - length)
                .map_err(|_| CapacityError::NoMemory)?;
        }
        pipe.capacity = capacity;
        Ok(capacity)
    }

    pub fn free(&self) -> usize {
        let pipe = self.pipe.borrow();
        pipe.capacity - pipe.bytes.len()
    }
    pub fn peer_closed(&self) -> bool {
        let pipe = self.pipe.borrow();
        if self.read {
            !pipe.writer
        } else {
            !pipe.reader
        }
    }
    pub fn write(&self, bytes: &[u8]) {
        self.pipe.borrow_mut().bytes.extend(bytes);
    }

    pub fn readiness(&self) -> u16 {
        let pipe = self.pipe.borrow();
        if self.read {
            u16::from(!pipe.bytes.is_empty()) | if pipe.writer { 0 } else { 0x10 }
        } else {
            (if pipe.capacity - pipe.bytes.len() >= PIPE_BUF {
                4
            } else {
                0
            }) | if pipe.reader { 0 } else { 8 }
        }
    }
}

impl Drop for PipeEnd {
    fn drop(&mut self) {
        let mut pipe = self.pipe.borrow_mut();
        if self.read {
            pipe.reader = false;
        } else {
            pipe.writer = false;
        }
    }
}
