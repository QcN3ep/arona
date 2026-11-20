// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::{FrameAccess, FrameAccessReference, FrameAllocation, MemoryError, ReferenceKind};
use crate::hardware::mmu::PAGE_SIZE;
use alloc::vec::Vec;

pub(crate) struct MemoryBuffer {
    frames: Vec<FrameAccessReference>,
    offset: usize,
    length: usize,
}

pub(crate) struct UserMemoryError(pub(crate) MemoryError);

impl MemoryBuffer {
    pub(super) fn new(offset: usize, length: usize) -> Result<Self, MemoryError> {
        let mut frames = Vec::new();
        let count = offset
            .checked_add(length)
            .ok_or(MemoryError::InvalidRequest)?
            .div_ceil(PAGE_SIZE);
        frames
            .try_reserve_exact(count)
            .map_err(|_| MemoryError::NoMemory)?;
        Ok(Self {
            frames,
            offset,
            length,
        })
    }

    pub(super) fn push(&mut self, frame: FrameAccessReference) {
        self.frames.push(frame);
    }

    pub(crate) fn allocation(
        allocation: &FrameAllocation,
        offset: usize,
        length: usize,
        write: bool,
    ) -> Result<Self, MemoryError> {
        let end = offset
            .checked_add(length)
            .ok_or(MemoryError::InvalidRequest)?;
        if end as u64
            > allocation
                .info()
                .frames
                .checked_mul(PAGE_SIZE as u64)
                .ok_or(MemoryError::InvalidRequest)?
        {
            return Err(MemoryError::InvalidRequest);
        }
        let mut buffer = Self::new(offset % PAGE_SIZE, length)?;
        if length != 0 {
            for frame in allocation
                .frames()
                .skip(offset / PAGE_SIZE)
                .take((end - 1) / PAGE_SIZE + 1 - offset / PAGE_SIZE)
            {
                buffer.push(allocation.access(
                    frame,
                    ReferenceKind::Temporary,
                    if write {
                        FrameAccess::Write
                    } else {
                        FrameAccess::Read
                    },
                )?);
            }
        }
        Ok(buffer)
    }

    pub(crate) fn read(&self, offset: usize, bytes: &mut [u8]) -> Result<(), MemoryError> {
        self.check(offset, bytes.len())?;
        let mut done = 0;
        while done < bytes.len() {
            let position = self.offset + offset + done;
            let inside = position % PAGE_SIZE;
            let count = (bytes.len() - done).min(PAGE_SIZE - inside);
            self.frames[position / PAGE_SIZE].read_bytes(inside, &mut bytes[done..done + count])?;
            done += count;
        }
        Ok(())
    }

    pub(crate) fn copy_to(&self, output: &Self) -> Result<(), MemoryError> {
        if self.length != output.length {
            return Err(MemoryError::InvalidRequest);
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(self.length)
            .map_err(|_| MemoryError::NoMemory)?;
        bytes.resize(self.length, 0);
        self.read(0, &mut bytes)?;
        output.write(0, &bytes)
    }

    pub(crate) fn write(&self, offset: usize, bytes: &[u8]) -> Result<(), MemoryError> {
        self.check(offset, bytes.len())?;
        let mut done = 0;
        while done < bytes.len() {
            let position = self.offset + offset + done;
            let inside = position % PAGE_SIZE;
            let count = (bytes.len() - done).min(PAGE_SIZE - inside);
            self.frames[position / PAGE_SIZE].write_bytes(inside, &bytes[done..done + count])?;
            done += count;
        }
        Ok(())
    }

    /// Atomically compares and exchanges one aligned 32-bit word in the buffer.
    pub(crate) fn compare_exchange_u32(
        &self,
        offset: usize,
        expected: u32,
        new: u32,
    ) -> Result<Option<u32>, MemoryError> {
        self.check(offset, 4)?;
        let position = self
            .offset
            .checked_add(offset)
            .ok_or(MemoryError::InvalidRequest)?;
        let index = position / PAGE_SIZE;
        let within = position % PAGE_SIZE;
        if within + 4 > PAGE_SIZE {
            return Err(MemoryError::InvalidRequest);
        }
        let frame = self.frames.get(index).ok_or(MemoryError::InvalidRequest)?;
        frame.compare_exchange_u32(within, expected, new)
    }

    fn check(&self, offset: usize, length: usize) -> Result<(), MemoryError> {
        if offset
            .checked_add(length)
            .is_none_or(|end| end > self.length)
        {
            Err(MemoryError::InvalidRequest)
        } else {
            Ok(())
        }
    }
}
