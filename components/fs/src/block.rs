// SPDX-License-Identifier: MPL-2.0

//! 块设备会话上的定长磁盘访问。

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
        let deadline = thread::now_ns()?
            .checked_add(10_000_000_000)
            .ok_or(Error::Unavailable)?;
        let completion = state.session.acknowledge(id, Some(deadline))?;
        state.failed = false;
        if completion.status != Status::Success || completion.bytes != BLOCK_SIZE {
            return Err(Error::Io);
        }
        state.buffer.read(0, output)?;
        Ok(())
    }
}
