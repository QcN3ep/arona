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
        let mut words = [0; PAGE_SIZE / 8];
        let mut done = 0;
        while done < bytes.len() {
            let position = self.offset + offset + done;
            let frame = &self.frames[position / PAGE_SIZE];
            let inside = position % PAGE_SIZE;
            let count = (bytes.len() - done).min(PAGE_SIZE - inside);
            let leading = inside % 8;
            let word_count = (leading + count).div_ceil(8);
            frame.read(inside / 8, &mut words[..word_count])?;
            for (index, word) in words[..word_count].iter().enumerate() {
                let start = if index == 0 { leading } else { 0 };
                let end = (leading + count - index * 8).min(8);
                let length = end - start;
                bytes[done..done + length].copy_from_slice(&word.to_le_bytes()[start..end]);
                done += length;
            }
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
        let mut words = [0; PAGE_SIZE / 8];
        let mut done = 0;
        while done < bytes.len() {
            let position = self.offset + offset + done;
            let frame = &self.frames[position / PAGE_SIZE];
            let inside = position % PAGE_SIZE;
            let count = (bytes.len() - done).min(PAGE_SIZE - inside);
            let leading = inside % 8;
            let word_count = (leading + count).div_ceil(8);
            if leading != 0 {
                frame.read(inside / 8, &mut words[..1])?;
            }
            if !(leading + count).is_multiple_of(8) && (word_count > 1 || leading == 0) {
                frame.read(
                    inside / 8 + word_count - 1,
                    &mut words[word_count - 1..word_count],
                )?;
            }
            for (index, word) in words[..word_count].iter_mut().enumerate() {
                let start = if index == 0 { leading } else { 0 };
                let end = (leading + count - index * 8).min(8);
                let length = end - start;
                let mut value = word.to_le_bytes();
                value[start..end].copy_from_slice(&bytes[done..done + length]);
                *word = u64::from_le_bytes(value);
                done += length;
            }
            frame.write(inside / 8, &words[..word_count])?;
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
