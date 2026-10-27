// SPDX-License-Identifier: MPL-2.0

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
            let frame = &self.frames[position / PAGE_SIZE];
            let inside = position % PAGE_SIZE;
            let count = (bytes.len() - done).min(8 - inside % 8);
            let mut word = [0];
            frame.read(inside / 8, &mut word)?;
            bytes[done..done + count]
                .copy_from_slice(&word[0].to_le_bytes()[inside % 8..inside % 8 + count]);
            done += count;
        }
        Ok(())
    }

    pub(crate) fn write(&self, offset: usize, bytes: &[u8]) -> Result<(), MemoryError> {
        self.check(offset, bytes.len())?;
        let mut done = 0;
        while done < bytes.len() {
            let position = self.offset + offset + done;
            let frame = &self.frames[position / PAGE_SIZE];
            let inside = position % PAGE_SIZE;
            let count = (bytes.len() - done).min(8 - inside % 8);
            let mut word = [0];
            if count != 8 {
                frame.read(inside / 8, &mut word)?;
            }
            let mut value = word[0].to_le_bytes();
            value[inside % 8..inside % 8 + count].copy_from_slice(&bytes[done..done + count]);
            frame.write(inside / 8, &[u64::from_le_bytes(value)])?;
            done += count;
        }
        Ok(())
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
