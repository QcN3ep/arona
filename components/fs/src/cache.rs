// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! File-service pages shared by byte I/O and mapped file clients.

use crate::{Attributes, DirEntry, Error, Metadata, ReadAt, Statistics, backend};
use alloc::{boxed::Box, collections::BTreeMap, rc::Rc, vec::Vec};
use core::cell::RefCell;
use runtime::{FrameAllocation, Handle, Rights, Task};

const PAGE_SIZE: usize = 4096;
const FRAME_RIGHTS: Rights = Rights::READ_WRITE
    .union(Rights::MAP)
    .union(Rights::EXECUTE)
    .union(Rights::TRANSFER)
    .union(Rights::DUPLICATE);

#[derive(Default)]
pub(crate) struct Cache {
    files: RefCell<BTreeMap<(u64, u64), Rc<Cached>>>,
}

pub(crate) struct Cached {
    file: Box<dyn backend::File>,
    state: RefCell<State>,
}

#[derive(Default)]
struct State {
    pages: BTreeMap<usize, Page>,
    status: Option<Handle<FrameAllocation>>,
    generation: u64,
    task: Option<&'static Handle<Task>>,
}

struct Page {
    frames: Handle<FrameAllocation>,
    writable: bool,
}

impl Cache {
    pub(crate) fn wrap(
        &self,
        file: Box<dyn backend::File>,
    ) -> Result<Box<dyn backend::File>, Error> {
        self.reclaim()?;
        let metadata = file.metadata()?;
        if metadata.mode & 0o170000 != 0o100000 {
            return Ok(file);
        }
        let mut files = self.files.borrow_mut();
        let entry = files
            .entry((metadata.device, metadata.inode))
            .or_insert_with(|| {
                Rc::new(Cached {
                    file,
                    state: RefCell::new(State::default()),
                })
            });
        Ok(Box::new(Rc::clone(entry)))
    }

    /// Flushes and releases files once their last session and mapping source close.
    pub(crate) fn reclaim(&self) -> Result<(), Error> {
        let mut files = self.files.borrow_mut();
        let keys: Vec<_> = files
            .iter()
            .filter(|(_, file)| Rc::strong_count(file) == 1)
            .map(|(key, _)| *key)
            .collect();
        for key in keys {
            files[&key].flush()?;
            files.remove(&key);
        }
        Ok(())
    }

    pub(crate) fn sync(&self) -> Result<(), Error> {
        for file in self.files.borrow().values() {
            file.flush()?;
        }
        self.reclaim()
    }
}

impl Cached {
    fn publish(&self, truncated: bool) -> Result<(), Error> {
        let mut state = self.state.borrow_mut();
        if truncated {
            state.generation = state.generation.checked_add(1).ok_or(Error::Unavailable)?;
        }
        let generation = state.generation;
        if let Some(status) = &mut state.status {
            status.write(0, &(self.file.len() as u64).to_le_bytes())?;
            status.write(8, &generation.to_le_bytes())?;
        }
        Ok(())
    }

    fn flush(&self) -> Result<(), Error> {
        let state = self.state.borrow();
        if !state.pages.values().any(|page| page.writable) {
            return Ok(());
        }
        let size = self.file.len();
        for (&offset, page) in &state.pages {
            if !page.writable {
                continue;
            }
            let frames = &page.frames;
            if offset >= size {
                continue;
            }
            let count = PAGE_SIZE.min(size - offset);
            let mut bytes = [0; PAGE_SIZE];
            frames.read(0, &mut bytes[..count])?;
            if self.file.write_at(offset, &bytes[..count])? != count {
                return Err(Error::Unavailable);
            }
        }
        self.file.fsync()
    }

    /// Holds write access before changing the backing store so executable aliases
    /// reject the operation before either the cache or file contents change.
    fn write_guards(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<Vec<runtime::memory::Mapping<'static>>, Error> {
        let state = self.state.borrow();
        let mut guards = Vec::new();
        for page in state
            .pages
            .range((offset & !(PAGE_SIZE - 1))..offset.saturating_add(length))
            .map(|(_, page)| page)
        {
            let task = state.task.ok_or(Error::InvalidData)?;
            guards.push(task.map(
                &page.frames,
                0,
                1,
                None,
                runtime::memory::Permissions::ReadWrite,
            )?);
        }
        Ok(guards)
    }

    fn update(&self, offset: usize, bytes: &[u8]) -> Result<(), Error> {
        let mut state = self.state.borrow_mut();
        let end = offset + bytes.len();
        for (&page, cached) in state.pages.range_mut((offset & !(PAGE_SIZE - 1))..end) {
            let start = offset.max(page);
            let limit = end.min(page + PAGE_SIZE);
            cached
                .frames
                .write(start - page, &bytes[start - offset..limit - offset])?;
        }
        Ok(())
    }
}

impl ReadAt for Rc<Cached> {
    fn len(&self) -> usize {
        self.file.len()
    }
    fn read_at(&self, offset: usize, output: &mut [u8]) -> Result<usize, Error> {
        let count = output.len().min(self.file.len().saturating_sub(offset));
        let state = self.state.borrow();
        let mut done = 0;
        while done < count {
            let position = offset + done;
            let page = position & !(PAGE_SIZE - 1);
            let length = (PAGE_SIZE - position % PAGE_SIZE).min(count - done);
            if let Some(cached) = state.pages.get(&page) {
                cached
                    .frames
                    .read(position - page, &mut output[done..done + length])?;
            } else if self
                .file
                .read_at(position, &mut output[done..done + length])?
                != length
            {
                return Err(Error::Unavailable);
            }
            done += length;
        }
        Ok(count)
    }
}

impl backend::File for Rc<Cached> {
    fn statistics(&self) -> Result<Statistics, Error> {
        self.file.statistics()
    }
    fn path(&self) -> Result<Vec<u8>, Error> {
        self.file.path()
    }
    fn check_writable(&self) -> Result<(), Error> {
        self.file.check_writable()
    }
    fn read_dir(&self) -> Result<Vec<DirEntry>, Error> {
        self.file.read_dir()
    }
    fn metadata(&self) -> Result<Metadata, Error> {
        self.file.metadata()
    }
    fn parent_metadata(&self) -> Result<Option<Metadata>, Error> {
        self.file.parent_metadata()
    }
    fn set_attributes(&self, attributes: Attributes) -> Result<(), Error> {
        self.file.set_attributes(attributes)
    }
    fn append(&self, bytes: &[u8]) -> Result<(usize, usize), Error> {
        let _guards = self.write_guards(self.file.len(), bytes.len())?;
        let (offset, count) = self.file.append(bytes)?;
        self.update(offset, &bytes[..count])?;
        self.publish(false)?;
        Ok((offset, count))
    }
    fn fsync(&self) -> Result<(), Error> {
        self.flush()
    }
    fn write_at(&self, offset: usize, bytes: &[u8]) -> Result<usize, Error> {
        let _guards = self.write_guards(offset, bytes.len())?;
        let count = self.file.write_at(offset, bytes)?;
        self.update(offset, &bytes[..count])?;
        self.publish(false)?;
        Ok(count)
    }
    fn truncate(&self, size: usize) -> Result<(), Error> {
        let _guards = if size < self.file.len() && !size.is_multiple_of(PAGE_SIZE) {
            self.write_guards(size, PAGE_SIZE - size % PAGE_SIZE)?
        } else {
            Vec::new()
        };
        self.file.truncate(size)?;
        let mut state = self.state.borrow_mut();
        state.pages.retain(|offset, _| *offset < size);
        if !size.is_multiple_of(PAGE_SIZE)
            && let Some(cached) = state.pages.get_mut(&(size & !(PAGE_SIZE - 1)))
        {
            cached.frames.write(
                size % PAGE_SIZE,
                &[0; PAGE_SIZE][..PAGE_SIZE - size % PAGE_SIZE],
            )?;
        }
        drop(state);
        self.publish(true)
    }
    fn hugepage_size(&self) -> u64 {
        self.file.hugepage_size()
    }
    fn map_huge(&self, offset: usize) -> Result<Handle<FrameAllocation>, Error> {
        self.file.map_huge(offset)
    }
    fn mapping_state(&self, task: &'static Handle<Task>) -> Result<Handle<FrameAllocation>, Error> {
        let mut state = self.state.borrow_mut();
        state.task = Some(task);
        if state.status.is_none() {
            let mut status =
                task.allocate(1, Rights::READ_WRITE | Rights::TRANSFER | Rights::DUPLICATE)?;
            status.write(0, &(self.file.len() as u64).to_le_bytes())?;
            status.write(8, &state.generation.to_le_bytes())?;
            state.status = Some(status);
        }
        Ok(state
            .status
            .as_ref()
            .unwrap()
            .try_clone(Rights::READ | Rights::TRANSFER)?)
    }
    fn map_page(
        &self,
        task: &'static Handle<Task>,
        offset: usize,
        writable: bool,
    ) -> Result<Handle<FrameAllocation>, Error> {
        if !offset.is_multiple_of(PAGE_SIZE) || offset >= self.file.len() {
            return Err(Error::InvalidRequest);
        }
        let mut state = self.state.borrow_mut();
        state.task = Some(task);
        if let alloc::collections::btree_map::Entry::Vacant(entry) = state.pages.entry(offset) {
            let mut bytes = [0; PAGE_SIZE];
            self.file.read_at(offset, &mut bytes)?;
            let mut frames = task.allocate(1, FRAME_RIGHTS)?;
            frames.write(0, &bytes)?;
            entry.insert(Page { frames, writable });
        }
        let page = state.pages.get_mut(&offset).unwrap();
        page.writable |= writable;
        Ok(page.frames.try_clone(FRAME_RIGHTS)?)
    }
}
