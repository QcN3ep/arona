// SPDX-License-Identifier: MPL-2.0

//! 块设备会话上的定长磁盘访问。

use alloc::vec::Vec;
use core::cell::RefCell;
use devmgr::block::{Session, Status};
use runtime::{FrameAllocation, Handle, Rights, Task, thread};

use crate::Error;

pub const BLOCK_SIZE: usize = 4096;

pub struct Disk {
    state: RefCell<State>,
    capacity: u64,
    read_only: bool,
    flush: bool,
    volumes: RefCell<Vec<(u64, u64)>>,
}

struct State {
    session: Session,
    buffer: Handle<FrameAllocation>,
    next: u64,
    failed: bool,
}

impl Disk {
    pub fn new(task: &Handle<Task>, session: Session) -> Result<Self, Error> {
        let info = session.info()?;
        let buffer = task.allocate(1, Rights::READ_WRITE | Rights::DUPLICATE | Rights::TRANSFER)?;
        Ok(Self {
            state: RefCell::new(State {
                session,
                buffer,
                next: 1,
                failed: false,
            }),
            capacity: info.capacity,
            read_only: info.read_only,
            flush: info.flush,
            volumes: RefCell::new(Vec::new()),
        })
    }

    pub fn capacity(&self) -> u64 {
        self.capacity
    }
    pub fn read_only(&self) -> bool {
        self.read_only
    }
    pub fn supports_flush(&self) -> bool {
        self.flush
    }

    pub(crate) fn claim(&self, start: u64, length: u64) -> Result<(), Error> {
        let end = start.checked_add(length).ok_or(Error::InvalidRequest)?;
        let mut volumes = self.volumes.borrow_mut();
        if volumes
            .iter()
            .any(|&(first, last)| start < last && first < end)
        {
            return Err(Error::Busy);
        }
        volumes.push((start, end));
        Ok(())
    }
    pub(crate) fn release(&self, start: u64) {
        self.volumes
            .borrow_mut()
            .retain(|&(first, _)| first != start);
    }

    pub fn flush(&self) -> Result<(), Error> {
        if !self.flush {
            return Err(Error::Unsupported);
        }
        let mut state = self.state.borrow_mut();
        if state.failed {
            return Err(Error::Unavailable);
        }
        let id = state.next;
        state.next = id.checked_add(1).ok_or(Error::Unavailable)?;
        state.failed = true;
        state.session.flush(id)?;
        state.complete(id, 0)
    }

    pub fn write(&self, offset: u64, input: &[u8; BLOCK_SIZE]) -> Result<(), Error> {
        if self.read_only {
            return Err(Error::ReadOnly);
        }
        if !offset.is_multiple_of(BLOCK_SIZE as u64)
            || offset
                .checked_add(BLOCK_SIZE as u64)
                .is_none_or(|end| end > self.capacity)
        {
            return Err(Error::InvalidRequest);
        }
        let mut state = self.state.borrow_mut();
        if state.failed {
            return Err(Error::Unavailable);
        }
        state.buffer.write(0, input)?;
        let id = state.next;
        state.next = id.checked_add(1).ok_or(Error::Unavailable)?;
        state.failed = true;
        state
            .session
            .write(id, offset, &state.buffer, 0, BLOCK_SIZE)?;
        state.complete(id, BLOCK_SIZE)
    }

    pub fn read(&self, offset: u64, output: &mut [u8; BLOCK_SIZE]) -> Result<(), Error> {
        if !offset.is_multiple_of(BLOCK_SIZE as u64)
            || offset
                .checked_add(BLOCK_SIZE as u64)
                .is_none_or(|end| end > self.capacity)
        {
            return Err(Error::InvalidRequest);
        }
        let mut state = self.state.borrow_mut();
        if state.failed {
            return Err(Error::Unavailable);
        }
        let id = state.next;
        state.next = id.checked_add(1).ok_or(Error::Unavailable)?;
        state.failed = true;
        state
            .session
            .read(id, offset, &state.buffer, 0, BLOCK_SIZE)?;
        state.complete(id, BLOCK_SIZE)?;
        state.buffer.read(0, output)?;
        Ok(())
    }
}

impl State {
    fn complete(&mut self, id: u64, length: usize) -> Result<(), Error> {
        let deadline = thread::now_ns()?
            .checked_add(10_000_000_000)
            .ok_or(Error::Unavailable)?;
        let completion = self.session.acknowledge(id, Some(deadline))?;
        self.failed = false;
        if completion.status != Status::Success || completion.bytes != length {
            return Err(Error::Io);
        }
        Ok(())
    }
}
