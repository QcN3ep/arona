// SPDX-License-Identifier: MPL-2.0

use core::ops::Range;
use runtime::{Handle, Rights, Task, TrapPort, memory::Permissions, process::SupervisedTask};

use crate::{
    Error,
    elf::{Image, PAGE_SIZE},
    protocol::Arguments,
};

pub(crate) struct AddressSpace {
    pub native: SupervisedTask,
    heap: Range<usize>,
}

impl AddressSpace {
    pub fn load(
        parent: &Handle<Task>,
        port: &Handle<TrapPort>,
        image: &Image<'_>,
        arguments: &Arguments,
    ) -> Result<(Self, usize), Error> {
        let start = image
            .segments
            .last()
            .ok_or(Error::InvalidExecutable)?
            .pages()
            .end;
        let address_space = Self {
            native: SupervisedTask::new(parent, port)?,
            heap: start..start,
        };
        for segment in &image.segments {
            let range = segment.pages();
            let count = (range.len() / PAGE_SIZE) as u64;
            let mut frames =
                parent.allocate(count, Rights::READ_WRITE | Rights::MAP | Rights::EXECUTE)?;
            frames.write(segment.memory.start - range.start, segment.data)?;
            address_space
                .native
                .map(&frames, 0, count, Some(range.start), segment.permissions)?;
        }
        let stack = crate::stack::build(&address_space.native, image, arguments)?;
        Ok((address_space, stack))
    }

    pub fn read(&self, address: usize, output: &mut [u8]) -> Result<(), runtime::Error> {
        self.native.task().read_memory(address, output)
    }

    pub fn map_anonymous(
        &self,
        address: Option<usize>,
        length: usize,
        permissions: Permissions,
        exact: bool,
    ) -> Result<usize, runtime::Error> {
        let reservation = self.native.task().reserve(length, PAGE_SIZE, address);
        let mut reservation = match reservation {
            Err(runtime::Error::Kernel(
                runtime::KernelError::Busy | runtime::KernelError::InvalidArgument,
            )) if !exact && address.is_some() => {
                self.native.task().reserve(length, PAGE_SIZE, None)?
            }
            result => result?,
        };
        let range = reservation.range().ok_or(runtime::Error::Protocol)?;
        reservation.release()?;
        self.map_zeroed(range.clone(), permissions)?;
        Ok(range.start)
    }

    pub fn unmap(&self, range: Range<usize>) -> Result<(), runtime::Error> {
        let bounds = self.native.task().address_range()?;
        if range.end > bounds.end {
            return Err(runtime::Error::Kernel(
                runtime::KernelError::InvalidArgument,
            ));
        }
        let range = range.start.max(bounds.start)..range.end.min(bounds.end);
        let mut cursor = range.start;
        while cursor < range.end {
            let Some(info) = self.native.task().query_next(cursor)? else {
                break;
            };
            if info.range.start >= range.end {
                break;
            }
            if info.permissions.is_none() {
                return Err(runtime::Error::Kernel(runtime::KernelError::Busy));
            }
            cursor = info.range.end;
        }
        cursor = range.start;
        while cursor < range.end {
            let Some(info) = self.native.task().query_next(cursor)? else {
                break;
            };
            if info.range.start >= range.end {
                break;
            }
            let start = cursor.max(info.range.start);
            let mut end = info.range.end.min(range.end);
            while end < range.end {
                let Some(next) = self.native.task().query_next(end)? else {
                    break;
                };
                if next.range.start != end {
                    break;
                }
                end = next.range.end.min(range.end);
            }
            self.native.unmap(start..end)?;
            cursor = end;
        }
        Ok(())
    }

    // REFERENCE: https://man7.org/linux/man-pages/man2/brk.2.html
    pub fn brk(&mut self, address: usize) -> Result<usize, runtime::Error> {
        if address < self.heap.start {
            return Ok(self.heap.end);
        }
        let Some(end) = address.checked_next_multiple_of(PAGE_SIZE) else {
            return Ok(self.heap.end);
        };
        let previous = self.heap.end.next_multiple_of(PAGE_SIZE);
        let changed = match end.cmp(&previous) {
            core::cmp::Ordering::Less => self.unmap(end..previous).is_ok(),
            core::cmp::Ordering::Equal => true,
            core::cmp::Ordering::Greater => self.grow_heap(previous..end)?,
        };
        if changed {
            self.heap.end = address;
        }
        Ok(self.heap.end)
    }

    fn grow_heap(&self, range: Range<usize>) -> Result<bool, runtime::Error> {
        let Ok(mut reservation) =
            self.native
                .task()
                .reserve(range.len(), PAGE_SIZE, Some(range.start))
        else {
            return Ok(false);
        };
        reservation.release()?;
        match self.map_zeroed(range, Permissions::ReadWrite) {
            Ok(()) => Ok(true),
            Err(runtime::Error::Kernel(
                runtime::KernelError::NoMemory
                | runtime::KernelError::Busy
                | runtime::KernelError::InvalidArgument,
            )) => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn map_zeroed(
        &self,
        range: Range<usize>,
        permissions: Permissions,
    ) -> Result<(), runtime::Error> {
        // 独立分配页框，使部分解除映射能逐页回收。
        for address in range.clone().step_by(PAGE_SIZE) {
            let result = self
                .native
                .task()
                .allocate(1, Rights::READ_WRITE | Rights::MAP | Rights::EXECUTE)
                .and_then(|frames| self.native.map(&frames, 0, 1, Some(address), permissions));
            if let Err(error) = result {
                if address != range.start {
                    self.native.unmap(range.start..address)?;
                }
                return Err(error);
            }
        }
        Ok(())
    }
}
