// SPDX-License-Identifier: MPL-2.0

use crate::{BorrowedHandle, Error, FrameAllocation, Handle, Rights, Task};
pub use abi::memory::Permissions;
use abi::method::{memory, task};
use core::ops::Range;

pub struct Mapping<'a> {
    task: BorrowedHandle<'a, Task>,
    range: Option<Range<usize>>,
}
pub struct Reservation<'a> {
    task: BorrowedHandle<'a, Task>,
    range: Option<Range<usize>>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MappingInfo {
    pub range: Range<usize>,
    pub permissions: Option<Permissions>,
}

impl Handle<Task> {
    pub fn read_memory(&self, address: usize, output: &mut [u8]) -> Result<(), Error> {
        self.borrow().invoke(
            task::READ_MEMORY,
            [
                address as u64,
                output.as_mut_ptr() as u64,
                output.len() as u64,
                0,
                0,
            ],
        )?;
        Ok(())
    }

    /// 写入其他 Task 的用户内存。
    pub fn write_memory(&self, address: usize, input: &[u8]) -> Result<(), Error> {
        self.borrow().invoke(
            task::WRITE_MEMORY,
            [
                address as u64,
                input.as_ptr() as u64,
                input.len() as u64,
                0,
                0,
            ],
        )?;
        Ok(())
    }

    pub fn allocate(&self, frames: u64, rights: Rights) -> Result<Handle<FrameAllocation>, Error> {
        let [raw, _] = self
            .borrow()
            .invoke(task::ALLOCATE, [frames, rights.bits(), 0, 0, 0])?;
        // SAFETY: ALLOCATE 成功须交付一个独占的页框分配句柄。
        Ok(unsafe { Handle::from_raw(abi::Handle::from_raw(raw)) })
    }
    pub fn map(
        &self,
        frames: &Handle<FrameAllocation>,
        offset: u64,
        count: u64,
        address: Option<usize>,
        permissions: Permissions,
    ) -> Result<Mapping<'_>, Error> {
        let [start, length] = self.borrow().invoke(
            task::MAP,
            [
                frames.borrow().raw().raw(),
                offset,
                count,
                address.unwrap_or(0) as u64,
                permissions as u64,
            ],
        )?;
        Ok(Mapping {
            task: self.borrow(),
            range: Some(decode_range(start, length)?),
        })
    }
    pub fn reserve(
        &self,
        length: usize,
        alignment: usize,
        address: Option<usize>,
    ) -> Result<Reservation<'_>, Error> {
        let [start, size] = self.borrow().invoke(
            task::RESERVE,
            [
                length as u64,
                alignment as u64,
                address.unwrap_or(0) as u64,
                0,
                0,
            ],
        )?;
        Ok(Reservation {
            task: self.borrow(),
            range: Some(decode_range(start, size)?),
        })
    }
    pub fn query(&self, address: usize) -> Result<Option<MappingInfo>, Error> {
        let [start, end] = self
            .borrow()
            .invoke(task::QUERY, [address as u64, 0, 0, 0, 0])?;
        if end == 0 {
            return Ok(None);
        }
        let permissions = if end & 0xfff == 0 {
            None
        } else {
            Some(Permissions::from_raw(end & 0xfff).ok_or(Error::Protocol)?)
        };
        Ok(Some(MappingInfo {
            range: start as usize..(end & !0xfff) as usize,
            permissions,
        }))
    }
}
impl Handle<FrameAllocation> {
    pub fn frames(&self) -> Result<u64, Error> {
        Ok(self.borrow().invoke(memory::SIZE, [0; 5])?[0])
    }
    pub fn read(&self, offset: usize, output: &mut [u8]) -> Result<(), Error> {
        self.borrow().invoke(
            memory::READ,
            [
                offset as u64,
                output.as_mut_ptr() as u64,
                output.len() as u64,
                0,
                0,
            ],
        )?;
        Ok(())
    }
    pub fn write(&mut self, offset: usize, input: &[u8]) -> Result<(), Error> {
        self.borrow().invoke(
            memory::WRITE,
            [
                offset as u64,
                input.as_ptr() as u64,
                input.len() as u64,
                0,
                0,
            ],
        )?;
        Ok(())
    }
}
impl Mapping<'_> {
    pub(crate) fn into_range(mut self) -> Result<Range<usize>, Error> {
        self.range
            .take()
            .ok_or(Error::Kernel(abi::Error::InvalidState))
    }

    pub fn range(&self) -> Option<Range<usize>> {
        self.range.clone()
    }
    pub fn protect(&mut self, permissions: Permissions) -> Result<(), Error> {
        let range = self
            .range()
            .ok_or(Error::Kernel(abi::Error::InvalidState))?;
        self.task.invoke(
            task::PROTECT,
            [
                range.start as u64,
                (range.end - range.start) as u64,
                permissions as u64,
                0,
                0,
            ],
        )?;
        Ok(())
    }
    pub fn unmap(&mut self) -> Result<(), Error> {
        if let Some(range) = &self.range {
            self.task.invoke(
                task::UNMAP,
                [
                    range.start as u64,
                    (range.end - range.start) as u64,
                    0,
                    0,
                    0,
                ],
            )?;
            self.range = None;
        }
        Ok(())
    }
}
impl Drop for Mapping<'_> {
    fn drop(&mut self) {
        let _ = self.unmap();
    }
}
impl Reservation<'_> {
    pub(crate) fn into_range(mut self) -> Result<Range<usize>, Error> {
        self.range
            .take()
            .ok_or(Error::Kernel(abi::Error::InvalidState))
    }

    pub fn range(&self) -> Option<Range<usize>> {
        self.range.clone()
    }
    pub fn release(&mut self) -> Result<(), Error> {
        if let Some(range) = &self.range {
            self.task.invoke(
                task::UNRESERVE,
                [
                    range.start as u64,
                    (range.end - range.start) as u64,
                    0,
                    0,
                    0,
                ],
            )?;
            self.range = None;
        }
        Ok(())
    }
}
impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

fn decode_range(start: u64, length: u64) -> Result<Range<usize>, Error> {
    let end = start.checked_add(length).ok_or(Error::Protocol)?;
    if start == 0 || length == 0 || start & 0xfff != 0 || end & 0xfff != 0 {
        return Err(Error::Protocol);
    }
    Ok(usize::try_from(start).map_err(|_| Error::Protocol)?
        ..usize::try_from(end).map_err(|_| Error::Protocol)?)
}
