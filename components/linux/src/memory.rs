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
            core::cmp::Ordering::Less => self.native.unmap(end..previous).is_ok(),
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
        // 独立分配页框，使收缩能逐页回收。
        for address in range.clone().step_by(PAGE_SIZE) {
            let result = self
                .native
                .task()
                .allocate(1, Rights::READ_WRITE | Rights::MAP)
                .and_then(|frames| {
                    self.native
                        .map(&frames, 0, 1, Some(address), Permissions::ReadWrite)
                });
            if result.is_err() {
                if address != range.start {
                    self.native.unmap(range.start..address)?;
                }
                return Ok(false);
            }
        }
        Ok(true)
    }
}
