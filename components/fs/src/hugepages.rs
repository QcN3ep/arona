// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Huge page backed files with `hugetlbfs`-style mapping semantics.
//!
//! The shared volume in [`crate::memory`] owns the inode tree; this backend stores files
//! as sparse vectors of 2 MiB blocks. Every block is one physically contiguous, 2 MiB
//! aligned frame allocation that is materialized when a block is first written or mapped.

use crate::{
    Error,
    memory::{self, Context, FileData},
};
use alloc::vec::Vec;
use runtime::{FrameAllocation, Handle, Rights, Task};

pub(crate) type Hugepages = memory::MemoryVolume<HugeFile>;

const DATA_PAGE_SIZE: u64 = 4096;
const HUGE_PAGE_SIZE: u64 = runtime::memory::HUGE_PAGE_SIZE as u64;
const HUGE_PAGE_FRAMES: u64 = HUGE_PAGE_SIZE / DATA_PAGE_SIZE;
// REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/include/uapi/linux/magic.h?h=v6.12
const MAGIC: u64 = 0x9584_58f6;

const FRAME_RIGHTS: Rights = Rights::READ_WRITE
    .union(Rights::MAP)
    .union(Rights::EXECUTE)
    .union(Rights::DUPLICATE)
    .union(Rights::TRANSFER);
const MAP_RIGHTS: Rights = Rights::READ_WRITE
    .union(Rights::MAP)
    .union(Rights::EXECUTE)
    .union(Rights::TRANSFER);

/// A materialized 2 MiB block and whether a client ever received its frames for mapping.
struct Page {
    frames: Handle<FrameAllocation>,
    mapped: bool,
}

impl Page {
    fn read(&self, offset: usize, output: &mut [u8]) -> Result<(), Error> {
        self.frames.read(offset, output)?;
        Ok(())
    }

    fn write_at(&mut self, offset: usize, input: &[u8]) -> Result<(), Error> {
        self.frames.write(offset, input)?;
        Ok(())
    }
}

pub(crate) struct HugeFile {
    size: u64,
    pages: Vec<Option<Page>>,
    task: &'static Handle<Task>,
}

impl FileData for HugeFile {
    const MAGIC: u64 = MAGIC;
    type Config = &'static Handle<Task>;

    fn new(task: Self::Config) -> Self {
        Self {
            size: 0,
            pages: Vec::new(),
            task,
        }
    }

    fn size(&self) -> u64 {
        self.size
    }

    fn data_size(&self) -> usize {
        self.pages
            .iter()
            .filter(|page| page.is_some())
            .count()
            .saturating_mul(HUGE_PAGE_SIZE as usize)
    }

    fn write(
        &mut self,
        context: &mut Context<'_>,
        offset: u64,
        input: &[u8],
    ) -> Result<usize, Error> {
        let end = offset
            .checked_add(input.len() as u64)
            .filter(|end| *end <= memory::MAX_FILE_SIZE)
            .ok_or(Error::FileTooLarge)?;
        let blocks =
            usize::try_from(end.div_ceil(HUGE_PAGE_SIZE)).map_err(|_| Error::FileTooLarge)?;
        let first = usize::try_from(offset / HUGE_PAGE_SIZE).map_err(|_| Error::FileTooLarge)?;
        let old_size = self.size;
        let old_len = self.pages.len();
        let mut created = Vec::new();
        let result = (|| {
            for index in first..blocks {
                if self.pages.get(index).and_then(Option::as_ref).is_none() {
                    self.materialize(context, index)?;
                    created.push(index);
                }
            }
            self.size = self.size.max(end.next_multiple_of(HUGE_PAGE_SIZE));
            let blocks =
                usize::try_from(self.size / HUGE_PAGE_SIZE).map_err(|_| Error::FileTooLarge)?;
            if self.pages.len() < blocks {
                self.pages.resize_with(blocks, || None);
            }
            let mut written = 0;
            while written < input.len() {
                let position = offset + written as u64;
                let index =
                    usize::try_from(position / HUGE_PAGE_SIZE).map_err(|_| Error::FileTooLarge)?;
                let inside = usize::try_from(position % HUGE_PAGE_SIZE).unwrap_or(0);
                let count = input.len().saturating_sub(written).min(
                    usize::try_from(HUGE_PAGE_SIZE)
                        .unwrap_or(usize::MAX)
                        .saturating_sub(inside),
                );
                let page = self
                    .pages
                    .get_mut(index)
                    .and_then(Option::as_mut)
                    .ok_or(Error::InvalidData)?;
                page.write_at(inside, &input[written..written + count])?;
                written += count;
            }
            Ok(written)
        })();
        if let Err(error) = result {
            for index in created {
                self.pages[index] = None;
                context.release(HUGE_PAGE_SIZE as usize);
            }
            self.pages.truncate(old_len);
            self.size = old_size;
            return Err(error);
        }
        Ok(input.len())
    }

    fn truncate(&mut self, context: &mut Context<'_>, size: u64) -> Result<(), Error> {
        if size > memory::MAX_FILE_SIZE {
            return Err(Error::FileTooLarge);
        }
        if !size.is_multiple_of(HUGE_PAGE_SIZE) {
            return Err(Error::InvalidRequest);
        }
        let blocks = usize::try_from(size / HUGE_PAGE_SIZE).map_err(|_| Error::FileTooLarge)?;
        let start = blocks.min(self.pages.len());
        self.retire(context, start);
        if self.pages.len() < blocks {
            self.pages.resize_with(blocks, || None);
        }
        self.size = size;
        Ok(())
    }

    fn release(&mut self, context: &mut Context<'_>) {
        self.retire(context, 0);
        self.size = 0;
    }

    fn read(&self, offset: u64, output: &mut [u8]) -> Result<usize, Error> {
        if offset >= self.size {
            return Ok(0);
        }
        let limit = usize::try_from(self.size - offset).unwrap_or(usize::MAX);
        let mut done = 0;
        while done < output.len() && done < limit {
            let position = offset + done as u64;
            let index = (position / HUGE_PAGE_SIZE) as usize;
            let inside = (position % HUGE_PAGE_SIZE) as usize;
            let count = (output.len() - done)
                .min(limit - done)
                .min(HUGE_PAGE_SIZE as usize - inside);
            match self.pages.get(index).and_then(Option::as_ref) {
                Some(page) => page.read(inside, &mut output[done..done + count])?,
                None => output[done..done + count].fill(0),
            }
            done += count;
        }
        Ok(done)
    }

    fn map(
        &mut self,
        context: &mut Context<'_>,
        offset: u64,
    ) -> Result<Handle<FrameAllocation>, Error> {
        if !offset.is_multiple_of(HUGE_PAGE_SIZE) || offset >= self.size {
            return Err(Error::InvalidRequest);
        }
        let index = usize::try_from(offset / HUGE_PAGE_SIZE).map_err(|_| Error::FileTooLarge)?;
        self.materialize(context, index)?;
        let page = self
            .pages
            .get_mut(index)
            .and_then(Option::as_mut)
            .ok_or(Error::InvalidData)?;
        let frames = page.frames.try_clone(MAP_RIGHTS)?;
        page.mapped = true;
        Ok(frames)
    }

    fn hugepage_size() -> u64 {
        HUGE_PAGE_SIZE
    }
}

impl HugeFile {
    /// Drops blocks from `start` onward, retiring mapped ones until their references are gone.
    fn retire(&mut self, context: &mut Context<'_>, start: usize) {
        let mut released = 0;
        for page in self.pages.drain(start..) {
            let Some(page) = page else {
                continue;
            };
            if page.mapped {
                context.retire(page.frames, HUGE_PAGE_SIZE as usize);
            } else {
                released += HUGE_PAGE_SIZE as usize;
            }
        }
        context.release(released);
    }

    fn materialize(&mut self, context: &mut Context<'_>, index: usize) -> Result<(), Error> {
        if self.pages.get(index).and_then(Option::as_ref).is_some() {
            return Ok(());
        }
        context.reserve(HUGE_PAGE_SIZE as usize)?;
        let frames = match self.task.allocate_contiguous(
            HUGE_PAGE_FRAMES,
            HUGE_PAGE_SIZE as usize,
            FRAME_RIGHTS,
        ) {
            Ok(frames) => frames,
            Err(error) => {
                context.release(HUGE_PAGE_SIZE as usize);
                return Err(error.into());
            }
        };
        if self.pages.len() <= index {
            self.pages.resize_with(index + 1, || None);
        }
        self.pages[index] = Some(Page {
            frames,
            mapped: false,
        });
        Ok(())
    }
}
