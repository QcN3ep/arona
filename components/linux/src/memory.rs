// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

// TODO: Support demand paging, page reclamation, and swapping.

use alloc::{collections::BTreeMap, rc::Rc, vec::Vec};
use core::{cell::RefCell, ops::Range};
use runtime::{
    FrameAllocation, Handle, Rights, Task, TrapPort, memory::Permissions, process::SupervisedTask,
};

use crate::{
    Error,
    elf::{Image, PAGE_SIZE},
    protocol::Arguments,
};

pub(crate) struct AddressSpace {
    pub native: SupervisedTask,
    heap: Range<usize>,
    pages: RefCell<BTreeMap<usize, Page>>,
}

#[derive(Clone)]
pub(super) struct Page {
    pub frames: Rc<Handle<FrameAllocation>>,
    pub offset: u64,
    pub backing: PageBacking,
}

#[derive(Clone, Copy)]
pub(super) enum PageBacking {
    Anonymous,
    Shared,
    File,
    BeyondEof(Permissions),
}

impl AddressSpace {
    pub fn load(
        parent: &Handle<Task>,
        port: &Handle<TrapPort>,
        image: &Image<'_>,
        arguments: &Arguments,
        random: Option<&[u8; 16]>,
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
            pages: RefCell::new(BTreeMap::new()),
        };
        for segment in &image.segments {
            let range = segment.pages();
            let count = (range.len() / PAGE_SIZE) as u64;
            let mut frames =
                parent.allocate(count, Rights::READ_WRITE | Rights::MAP | Rights::EXECUTE)?;
            frames.write(segment.memory.start - range.start, segment.data)?;
            address_space.map_frames(
                Rc::new(frames),
                0,
                count,
                range.start,
                segment.permissions,
                PageBacking::File,
            )?;
        }
        let stack = crate::stack::build(&address_space, image, arguments, random)?;
        Ok((address_space, stack))
    }

    pub fn read(&self, address: usize, output: &mut [u8]) -> Result<(), runtime::Error> {
        self.native.task().read_memory(address, output)
    }

    pub fn write(&self, address: usize, input: &[u8]) -> Result<(), runtime::Error> {
        self.native.task().write_memory(address, input)
    }

    pub fn map_anonymous(
        &self,
        address: Option<usize>,
        length: usize,
        permissions: Permissions,
        exact: bool,
        replace: bool,
        shared: bool,
    ) -> Result<usize, runtime::Error> {
        if replace {
            let address = address.ok_or(runtime::Error::Kernel(
                runtime::KernelError::InvalidArgument,
            ))?;
            let pages = self.zeroed_pages(length, shared)?;
            self.replace_pages(address, pages, permissions)?;
            return Ok(address);
        }
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
        self.map_zeroed(range.clone(), permissions, shared)?;
        Ok(range.start)
    }

    pub fn map_prepared(
        &self,
        address: Option<usize>,
        pages: Vec<Page>,
        permissions: Permissions,
        exact: bool,
        replace: bool,
    ) -> Result<usize, runtime::Error> {
        if replace {
            let address = address.ok_or(runtime::Error::Kernel(
                runtime::KernelError::InvalidArgument,
            ))?;
            self.replace_pages(address, pages, permissions)?;
            return Ok(address);
        }
        let length = pages
            .len()
            .checked_mul(PAGE_SIZE)
            .ok_or(runtime::Error::Kernel(runtime::KernelError::NoMemory))?;
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
        self.install(range.start, pages, permissions)?;
        Ok(range.start)
    }

    fn zeroed_pages(&self, length: usize, shared: bool) -> Result<Vec<Page>, runtime::Error> {
        let mut pages = Vec::with_capacity(length / PAGE_SIZE);
        for _ in (0..length).step_by(PAGE_SIZE) {
            let frames = self
                .native
                .task()
                .allocate(1, Rights::READ_WRITE | Rights::MAP | Rights::EXECUTE)?;
            pages.push(Page {
                frames: Rc::new(frames),
                offset: 0,
                backing: if shared {
                    PageBacking::Shared
                } else {
                    PageBacking::Anonymous
                },
            });
        }
        Ok(pages)
    }

    fn replace_pages(
        &self,
        start: usize,
        pages: Vec<Page>,
        permissions: Permissions,
    ) -> Result<(), runtime::Error> {
        let length = pages
            .len()
            .checked_mul(PAGE_SIZE)
            .ok_or(runtime::Error::Kernel(runtime::KernelError::NoMemory))?;
        let range = start
            ..start.checked_add(length).ok_or(runtime::Error::Kernel(
                runtime::KernelError::InvalidArgument,
            ))?;
        let saved: Vec<(usize, Page, Permissions)> = {
            let records = self.pages.borrow();
            records
                .range(range.clone())
                .map(|(address, page)| {
                    let permissions = self
                        .native
                        .task()
                        .query(*address)
                        .ok()
                        .flatten()
                        .and_then(|info| info.permissions)
                        .unwrap_or(Permissions::ReadWrite);
                    (*address, page.clone(), permissions)
                })
                .collect()
        };
        self.unmap(range)?;
        if let Err(error) = self.install(start, pages, permissions) {
            // 新映射安装失败时恢复被替换的旧页，避免调用方丢失原有映射。
            for (address, page, permissions) in saved {
                let effective = if matches!(page.backing, PageBacking::BeyondEof(_)) {
                    Permissions::NoAccess
                } else {
                    permissions
                };
                if self
                    .native
                    .map(&page.frames, page.offset, 1, Some(address), effective)
                    .is_ok()
                {
                    self.pages.borrow_mut().insert(address, page);
                }
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn protect(
        &self,
        range: Range<usize>,
        permissions: Permissions,
    ) -> Result<(), runtime::Error> {
        let mut changes = Vec::new();
        let mut cursor = range.start;
        while cursor < range.end {
            let info = self
                .native
                .task()
                .query(cursor)?
                .ok_or(runtime::Error::Kernel(runtime::KernelError::BadAddress))?;
            let previous = info
                .permissions
                .ok_or(runtime::Error::Kernel(runtime::KernelError::Busy))?;
            let page = self
                .pages
                .borrow()
                .get(&cursor)
                .cloned()
                .ok_or(runtime::Error::Protocol)?;
            changes.push((
                cursor,
                previous,
                if matches!(page.backing, PageBacking::BeyondEof(_)) {
                    Permissions::NoAccess
                } else {
                    permissions
                },
            ));
            cursor += PAGE_SIZE;
        }
        for (index, (address, _, next)) in changes.iter().enumerate() {
            if let Err(error) = self.native.protect(*address..*address + PAGE_SIZE, *next) {
                for (address, previous, _) in changes[..index].iter().rev() {
                    self.native
                        .protect(*address..*address + PAGE_SIZE, *previous)?;
                }
                return Err(error);
            }
        }
        for (_, page) in self.pages.borrow_mut().range_mut(range) {
            if matches!(page.backing, PageBacking::BeyondEof(_)) {
                page.backing = PageBacking::BeyondEof(permissions);
            }
        }
        Ok(())
    }

    pub fn discard(&self, range: Range<usize>) -> Result<(), runtime::Error> {
        let mut prepared = Vec::new();
        for address in range.clone().step_by(PAGE_SIZE) {
            let info = self
                .native
                .task()
                .query(address)?
                .ok_or(runtime::Error::Kernel(runtime::KernelError::BadAddress))?;
            let permissions = info
                .permissions
                .ok_or(runtime::Error::Kernel(runtime::KernelError::Busy))?;
            let page = self
                .pages
                .borrow()
                .get(&address)
                .cloned()
                .ok_or(runtime::Error::Protocol)?;
            if matches!(page.backing, PageBacking::File | PageBacking::BeyondEof(_)) {
                return Err(runtime::Error::Kernel(runtime::KernelError::Unsupported));
            }
            if matches!(page.backing, PageBacking::Anonymous) {
                let frames = self
                    .native
                    .task()
                    .allocate(1, Rights::READ_WRITE | Rights::MAP | Rights::EXECUTE)?;
                prepared.push((address, page, Rc::new(frames), permissions));
            }
        }
        for (address, previous, frames, permissions) in prepared {
            self.native.unmap(address..address + PAGE_SIZE)?;
            if let Err(error) =
                self.map_frames(frames, 0, 1, address, permissions, PageBacking::Anonymous)
            {
                self.map_frames(
                    previous.frames,
                    previous.offset,
                    1,
                    address,
                    permissions,
                    previous.backing,
                )?;
                return Err(error);
            }
        }
        Ok(())
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
            self.pages
                .borrow_mut()
                .retain(|address, _| *address < start || *address >= end);
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
        match self.map_zeroed(range, Permissions::ReadWrite, false) {
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
        shared: bool,
    ) -> Result<(), runtime::Error> {
        let prepared = self.zeroed_pages(range.len(), shared)?;
        self.install(range.start, prepared, permissions)
    }

    pub fn map_frames(
        &self,
        frames: Rc<Handle<FrameAllocation>>,
        offset: u64,
        count: u64,
        address: usize,
        permissions: Permissions,
        backing: PageBacking,
    ) -> Result<(), runtime::Error> {
        self.native
            .map(&frames, offset, count, Some(address), permissions)?;
        let mut pages = self.pages.borrow_mut();
        for index in 0..count {
            pages.insert(
                address + index as usize * PAGE_SIZE,
                Page {
                    frames: Rc::clone(&frames),
                    offset: offset + index,
                    backing,
                },
            );
        }
        Ok(())
    }

    fn install(
        &self,
        start: usize,
        pages: Vec<Page>,
        permissions: Permissions,
    ) -> Result<(), runtime::Error> {
        for (index, page) in pages.into_iter().enumerate() {
            let address = start + index * PAGE_SIZE;
            let effective = if matches!(page.backing, PageBacking::BeyondEof(_)) {
                Permissions::NoAccess
            } else {
                permissions
            };
            if let Err(error) =
                self.native
                    .map(&page.frames, page.offset, 1, Some(address), effective)
            {
                if address != start {
                    self.unmap(start..address)?;
                }
                return Err(error);
            }
            self.pages.borrow_mut().insert(address, page);
        }
        Ok(())
    }

    pub fn bus_fault(&self, address: usize, access: u64) -> bool {
        self.pages
            .borrow()
            .get(&(address & !(PAGE_SIZE - 1)))
            .and_then(|page| match page.backing {
                PageBacking::BeyondEof(permissions) => Some(permissions),
                _ => None,
            })
            .is_some_and(|permissions| access <= 2 && permissions as u64 & (1 << access) != 0)
    }
}

impl AddressSpace {
    pub fn futex_key(
        &self,
        pid: usize,
        address: usize,
        private: bool,
    ) -> Result<(crate::process::futex::Key, Rc<Handle<FrameAllocation>>), crate::syscall::abi::Errno>
    {
        use crate::syscall::abi::{EFAULT, EINVAL};
        if address & 3 != 0 {
            return Err(EINVAL);
        }
        let mut value = [0; 4];
        self.read(address, &mut value).map_err(|_| EFAULT)?;
        let pages = self.pages.borrow();
        let page = pages.get(&(address & !(PAGE_SIZE - 1))).ok_or(EFAULT)?;
        let key = if private {
            (pid, address, 0)
        } else {
            (
                0,
                Rc::as_ptr(&page.frames) as usize,
                page.offset as usize * PAGE_SIZE + address % PAGE_SIZE,
            )
        };
        Ok((key, Rc::clone(&page.frames)))
    }
}

impl AddressSpace {
    // TODO: Share private pages through copy-on-write instead of copying them page by page.
    pub fn fork(
        &self,
        parent: &Handle<Task>,
        port: &Handle<TrapPort>,
    ) -> Result<Self, runtime::Error> {
        let child = Self {
            native: SupervisedTask::new(parent, port)?,
            heap: self.heap.clone(),
            pages: RefCell::new(BTreeMap::new()),
        };
        let bounds = self.native.task().address_range()?;
        let mut cursor = bounds.start;
        while cursor < bounds.end {
            let Some(info) = self.native.task().query_next(cursor)? else {
                break;
            };
            cursor = info.range.end;
            if info.permissions.is_none() {
                child
                    .native
                    .reserve(info.range.len(), PAGE_SIZE, Some(info.range.start))?;
            }
        }
        for (&address, page) in self.pages.borrow().iter() {
            let permissions = self
                .native
                .task()
                .query(address)?
                .and_then(|info| info.permissions)
                .ok_or(runtime::Error::Protocol)?;
            let (frames, offset) = if matches!(page.backing, PageBacking::Shared) {
                (Rc::clone(&page.frames), page.offset)
            } else {
                let mut bytes = [0; PAGE_SIZE];
                page.frames
                    .read(page.offset as usize * PAGE_SIZE, &mut bytes)?;
                let mut frames =
                    parent.allocate(1, Rights::READ_WRITE | Rights::MAP | Rights::EXECUTE)?;
                frames.write(0, &bytes)?;
                (Rc::new(frames), 0)
            };
            child.map_frames(frames, offset, 1, address, permissions, page.backing)?;
        }
        Ok(child)
    }
}

impl AddressSpace {
    pub fn map_code(&self, bytes: &[u8]) -> Result<usize, runtime::Error> {
        let count = bytes.len().div_ceil(PAGE_SIZE) as u64;
        let mut frames = self
            .native
            .task()
            .allocate(count, Rights::READ_WRITE | Rights::MAP | Rights::EXECUTE)?;
        frames.write(0, bytes)?;
        let mut reservation =
            self.native
                .task()
                .reserve(count as usize * PAGE_SIZE, PAGE_SIZE, None)?;
        let address = reservation.range().ok_or(runtime::Error::Protocol)?.start;
        reservation.release()?;
        self.map_frames(
            Rc::new(frames),
            0,
            count,
            address,
            Permissions::ReadExecute,
            PageBacking::File,
        )?;
        Ok(address)
    }
}
