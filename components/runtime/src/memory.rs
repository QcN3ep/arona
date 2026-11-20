// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use crate::{BorrowedHandle, Error, FrameAllocation, Handle, Rights, Task};
pub use abi::memory::{HUGE_PAGE_SIZE, Permissions};
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
    pub leaf_level: u64,
}

/// Convenience access checks for native mapping permissions.
pub trait PermissionsExt {
    fn writable(self) -> bool;
    fn executable(self) -> bool;
}
impl PermissionsExt for Permissions {
    fn writable(self) -> bool {
        matches!(self, Self::ReadWrite | Self::ReadWriteExecute)
    }
    fn executable(self) -> bool {
        matches!(
            self,
            Self::ExecuteOnly | Self::ReadExecute | Self::ReadWriteExecute
        )
    }
}

impl Handle<Task> {
    pub fn address_range(&self) -> Result<Range<usize>, Error> {
        let region = self
            .address_space_query(abi::memory::AddressMode::Bounds, 0)?
            .ok_or(Error::Protocol)?;
        if region.kind != abi::memory::RegionKind::Bounds {
            return Err(Error::Protocol);
        }
        decode_range(region.start, region.end - region.start)
    }

    pub fn read_memory(&self, address: usize, output: &mut [u8]) -> Result<(), Error> {
        // SAFETY: The output slice is exclusively borrowed for the synchronous copy from the target Task.
        unsafe {
            self.borrow().invoke(
                task::READ_MEMORY,
                [
                    address as u64,
                    output.as_mut_ptr() as u64,
                    output.len() as u64,
                    0,
                    0,
                ],
            )
        }?;
        Ok(())
    }

    /// Write to another Task's user memory.
    pub fn write_memory(&self, address: usize, input: &[u8]) -> Result<(), Error> {
        // SAFETY: The input slice remains borrowed for the synchronous copy to the target Task.
        unsafe {
            self.borrow().invoke(
                task::WRITE_MEMORY,
                [
                    address as u64,
                    input.as_ptr() as u64,
                    input.len() as u64,
                    0,
                    0,
                ],
            )
        }?;
        Ok(())
    }

    /// Atomically compares and exchanges one aligned user word in another Task.
    ///
    /// Returns the previous word when the exchange happened, `None` when the
    /// current value differed from `expected`, and an error only when the word
    /// is not mapped or not writable.
    pub fn compare_exchange_memory(
        &self,
        address: usize,
        expected: u32,
        new: u32,
    ) -> Result<Option<u32>, Error> {
        // SAFETY: The address is checked in the target Task; this request passes no local buffer.
        let [old, swapped] = unsafe {
            self.borrow().invoke(
                task::ATOMIC_COMPARE_EXCHANGE,
                [address as u64, u64::from(expected), u64::from(new), 0, 0],
            )
        }?;
        Ok((swapped != 0).then_some(old as u32))
    }

    pub fn allocate(&self, frames: u64, rights: Rights) -> Result<Handle<FrameAllocation>, Error> {
        // SAFETY: ALLOCATE takes only handles and scalar values, with no local buffers.
        let [raw, _] = unsafe {
            self.borrow()
                .invoke(task::ALLOCATE, [frames, rights.bits(), 0, 0, 0])
        }?;
        // SAFETY: A successful ALLOCATE must deliver an exclusive page frame allocation handle.
        Ok(unsafe { Handle::from_raw(abi::Handle::from_raw(raw)) })
    }

    /// Allocates physically contiguous frames aligned to `alignment` bytes.
    pub fn allocate_contiguous(
        &self,
        frames: u64,
        alignment: usize,
        rights: Rights,
    ) -> Result<Handle<FrameAllocation>, Error> {
        // SAFETY: ALLOCATE takes only handles and scalar values, with no local buffers.
        let [raw, _] = unsafe {
            self.borrow().invoke(
                task::ALLOCATE,
                [frames, rights.bits(), alignment as u64, 0, 0],
            )
        }?;
        // SAFETY: A successful ALLOCATE must deliver an exclusive page frame allocation handle.
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
        self.map_at_level(frames, offset, count, address, permissions, 0)
    }

    /// Maps frames using leaf level one, with the layout's alignment and contiguity requirements.
    pub fn map_huge(
        &self,
        frames: &Handle<FrameAllocation>,
        offset: u64,
        count: u64,
        address: Option<usize>,
        permissions: Permissions,
    ) -> Result<Mapping<'_>, Error> {
        self.map_at_level(frames, offset, count, address, permissions, 1)
    }

    fn map_at_level(
        &self,
        frames: &Handle<FrameAllocation>,
        offset: u64,
        count: u64,
        address: Option<usize>,
        permissions: Permissions,
        level: u64,
    ) -> Result<Mapping<'_>, Error> {
        let mode = abi::memory::MapMode {
            permissions,
            leaf_level: u8::try_from(level)
                .map_err(|_| Error::Kernel(abi::Error::InvalidArgument))?,
        };
        // SAFETY: MAP takes only handles and scalar values, with no local buffers.
        let [start, length] = unsafe {
            self.borrow().invoke(
                task::MAP,
                [
                    frames.borrow().raw().raw(),
                    offset,
                    count,
                    address.unwrap_or(0) as u64,
                    mode.encode(),
                ],
            )
        }?;
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
        // SAFETY: RESERVE takes only handles and scalar values, with no local buffers.
        let [start, size] = unsafe {
            self.borrow().invoke(
                task::RESERVE,
                [
                    length as u64,
                    alignment as u64,
                    address.unwrap_or(0) as u64,
                    0,
                    0,
                ],
            )
        }?;
        Ok(Reservation {
            task: self.borrow(),
            range: Some(decode_range(start, size)?),
        })
    }
    pub fn query(&self, address: usize) -> Result<Option<MappingInfo>, Error> {
        self.query_mapping(address, abi::memory::AddressMode::Containing)
    }

    /// Query the region covering address or the first region after it.
    pub fn query_next(&self, address: usize) -> Result<Option<MappingInfo>, Error> {
        self.query_mapping(address, abi::memory::AddressMode::AtOrAfter)
    }

    fn query_mapping(
        &self,
        address: usize,
        mode: abi::memory::AddressMode,
    ) -> Result<Option<MappingInfo>, Error> {
        let Some(region) = self.address_space_query(mode, address as u64)? else {
            return Ok(None);
        };
        if region.kind == abi::memory::RegionKind::Bounds {
            return Err(Error::Protocol);
        }
        Ok(Some(MappingInfo {
            range: decode_range(region.start, region.end - region.start)?,
            permissions: region.permissions,
            leaf_level: region.leaf_level,
        }))
    }

    fn address_space_query(
        &self,
        mode: abi::memory::AddressMode,
        address: u64,
    ) -> Result<Option<abi::memory::AddressRegion>, Error> {
        let query = abi::memory::AddressQuery { mode, address }.encode();
        let mut output = [0; abi::memory::AddressRegion::SIZE];
        // SAFETY: The query stays borrowed and the output buffer stays exclusively borrowed for the call.
        let [written, _] = unsafe {
            self.borrow().invoke(
                abi::method::INFO,
                [
                    abi::info::task::ADDRESS_SPACE,
                    query.as_ptr() as u64,
                    query.len() as u64,
                    output.as_mut_ptr() as u64,
                    output.len() as u64,
                ],
            )
        }?;
        if written == 0 {
            return Ok(None);
        }
        if written != output.len() as u64 {
            return Err(Error::Protocol);
        }
        abi::memory::AddressRegion::decode(&output)
            .map(Some)
            .map_err(|_| Error::Protocol)
    }
}
/// Live references to a frame allocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AllocationState {
    /// Kernel mapping references that keep the frames installed in page tables.
    pub mappings: u64,
    /// Owned capabilities, including in-flight transfers and excluding temporary borrows.
    pub handle_owners: u64,
}

impl Handle<FrameAllocation> {
    pub fn frames(&self) -> Result<u64, Error> {
        Ok(self.borrow().query(abi::info::memory::SIZE)?[0])
    }

    pub fn state(&self) -> Result<AllocationState, Error> {
        let [mappings, handle_owners] = self.borrow().query(abi::info::memory::USAGE)?;
        Ok(AllocationState {
            mappings,
            handle_owners,
        })
    }
    pub fn read(&self, offset: usize, output: &mut [u8]) -> Result<(), Error> {
        // SAFETY: The output buffer remains exclusively borrowed until the call returns.
        unsafe {
            self.borrow().invoke(
                memory::READ,
                [
                    offset as u64,
                    output.as_mut_ptr() as u64,
                    output.len() as u64,
                    0,
                    0,
                ],
            )
        }?;
        Ok(())
    }
    pub fn write(&mut self, offset: usize, input: &[u8]) -> Result<(), Error> {
        self.borrow().write(offset, input)
    }
}

impl BorrowedHandle<'_, FrameAllocation> {
    /// Copies bytes into the allocation without taking ownership of its handle.
    pub fn write(&mut self, offset: usize, input: &[u8]) -> Result<(), Error> {
        // SAFETY: The input buffer remains borrowed until the call returns.
        unsafe {
            self.invoke(
                memory::WRITE,
                [
                    offset as u64,
                    input.as_ptr() as u64,
                    input.len() as u64,
                    0,
                    0,
                ],
            )
        }?;
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
        // SAFETY: PROTECT takes only handles and scalar values, with no local buffers.
        unsafe {
            self.task.invoke(
                task::PROTECT,
                [
                    range.start as u64,
                    (range.end - range.start) as u64,
                    permissions as u64,
                    0,
                    0,
                ],
            )
        }?;
        Ok(())
    }
    pub fn unmap(&mut self) -> Result<(), Error> {
        if let Some(range) = &self.range {
            // SAFETY: UNMAP takes only handles and scalar values, with no local buffers.
            unsafe {
                self.task.invoke(
                    task::UNMAP,
                    [
                        range.start as u64,
                        (range.end - range.start) as u64,
                        0,
                        0,
                        0,
                    ],
                )
            }?;
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
            // SAFETY: UNRESERVE takes only handles and scalar values, with no local buffers.
            unsafe {
                self.task.invoke(
                    task::UNRESERVE,
                    [
                        range.start as u64,
                        (range.end - range.start) as u64,
                        0,
                        0,
                        0,
                    ],
                )
            }?;
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
