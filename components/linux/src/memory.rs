// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

// TODO: Support demand paging, page reclamation, and swapping.

use alloc::{collections::BTreeMap, rc::Rc, vec::Vec};
use core::{cell::RefCell, ops::Range};
use runtime::{
    FrameAllocation, Handle, Rights, Task, TrapPort, memory::Permissions, process::SupervisedTask,
};

use crate::{
    Error,
    elf::{ImageSet, PAGE_SIZE},
    protocol::Arguments,
};

const HUGE_PAGE_SIZE: usize = runtime::memory::HUGE_PAGE_SIZE;
const HUGE_FRAMES: u64 = (HUGE_PAGE_SIZE / PAGE_SIZE) as u64;

pub(crate) struct AddressSpace {
    pub native: Rc<SupervisedTask>,
    heap: Range<usize>,
    pages: RefCell<BTreeMap<usize, Page>>,
    stack_origin: Rc<Origin>,
    heap_origin: Rc<Origin>,
    /// Ranges held by an in-flight huge mapping, shared with its [`Reservation`].
    ///
    /// This is not an owner: the guard releases the range and removes it here. The
    /// set remains readable while the guard is alive so a fork can leave an
    /// in-flight reservation out of its child.
    pending: Rc<RefCell<Vec<Range<usize>>>>,
}

/// Owns a reserved range until installation releases it.
///
/// The guard keeps the task alive and releases the range when dropped. The range
/// also stays in the address space's pending set until the kernel confirms the
/// unreserve, which lets [`AddressSpace::clone_space`] leave an in-flight
/// reservation out of a child. If the unreserve itself fails, the range stays
/// reserved until the task exits; the guard drops the error because it has no
/// caller left to report it.
pub(super) struct Reservation {
    task: Rc<SupervisedTask>,
    pending: Rc<RefCell<Vec<Range<usize>>>>,
    range: Option<Range<usize>>,
}

impl Reservation {
    fn new(
        task: Rc<SupervisedTask>,
        pending: Rc<RefCell<Vec<Range<usize>>>>,
        range: Range<usize>,
    ) -> Self {
        pending.borrow_mut().push(range.clone());
        Self {
            task,
            pending,
            range: Some(range),
        }
    }

    pub(super) fn range(&self) -> Option<Range<usize>> {
        self.range.clone()
    }

    pub(super) fn release(&mut self) -> Result<(), runtime::Error> {
        let Some(range) = self.range.clone() else {
            return Ok(());
        };
        self.task.unreserve(range.clone())?;
        self.range = None;
        self.pending.borrow_mut().retain(|entry| *entry != range);
        Ok(())
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

#[derive(Clone)]
pub(super) struct Page {
    pub frames: Rc<Handle<FrameAllocation>>,
    pub offset: u64,
    pub backing: PageBacking,
    pub huge: bool,
    pub origin: Option<Rc<Origin>>,
    /// File offset of this page for file-backed origins, otherwise zero.
    pub file_offset: u64,
}

/// Source of a page range, rendered in `/proc/self/maps`.
#[derive(Clone)]
pub(super) enum Origin {
    File(Rc<Vec<u8>>),
    Stack,
    Heap,
}

/// A contiguous mapped range with uniform permissions and origin.
pub(super) struct Region {
    pub start: usize,
    pub end: usize,
    pub permissions: Permissions,
    pub origin: Option<Rc<Origin>>,
    pub file_offset: u64,
}

#[derive(Clone, Copy)]
pub(super) enum PageBacking {
    Anonymous,
    Shared,
    SystemV(usize),
    File,
    ReadOnlyFile,
    BeyondEof(Permissions),
}

/// A mapped page together with the permissions observed in the native page table.
pub(super) struct MappedPage {
    pub address: usize,
    pub page: Page,
    pub permissions: Permissions,
}

fn effective_permissions(page: &Page, permissions: Permissions) -> Permissions {
    if matches!(page.backing, PageBacking::BeyondEof(_)) {
        Permissions::NoAccess
    } else {
        permissions
    }
}

/// One fully materialized 2 MiB block of a huge mapping.
pub(super) struct HugeBlock {
    pub address: usize,
    pub frames: Rc<Handle<FrameAllocation>>,
    pub permissions: Permissions,
    pub backing: PageBacking,
}

/// A replaced mapping record saved before its range is removed, so a failed
/// replacement can put the previous address space back.
struct SavedPage {
    range: Range<usize>,
    page: Page,
    permissions: Permissions,
}

impl AddressSpace {
    pub fn load(
        parent: &Handle<Task>,
        port: &Handle<TrapPort>,
        images: &ImageSet<'_>,
        arguments: &Arguments,
        random: Option<&[u8; 16]>,
        stack: usize,
    ) -> Result<(Self, usize), Error> {
        let main_path = Rc::new(arguments.path.clone());
        let interpreter_path = images.image.interpreter.map(|path| Rc::new(path.to_vec()));
        let mut segments: Vec<(&crate::elf::Segment<'_>, Rc<Vec<u8>>)> = images
            .image
            .segments
            .iter()
            .map(|segment| (segment, Rc::clone(&main_path)))
            .collect();
        if let Some(interpreter) = &images.interpreter {
            let path = interpreter_path.clone().ok_or(Error::InvalidExecutable)?;
            segments.extend(
                interpreter
                    .segments
                    .iter()
                    .map(|segment| (segment, Rc::clone(&path))),
            );
        }
        segments.sort_unstable_by_key(|(segment, _)| segment.memory.start);
        for pair in segments.windows(2) {
            if pair[0].0.pages().end > pair[1].0.pages().start {
                return Err(Error::InvalidExecutable);
            }
        }
        let start = segments
            .last()
            .ok_or(Error::InvalidExecutable)?
            .0
            .pages()
            .end;
        let address_space = Self {
            native: Rc::new(SupervisedTask::new(parent, port)?),
            heap: start..start,
            pages: RefCell::new(BTreeMap::new()),
            stack_origin: Rc::new(Origin::Stack),
            heap_origin: Rc::new(Origin::Heap),
            pending: Rc::new(RefCell::new(Vec::new())),
        };
        for (segment, path) in segments {
            let range = segment.pages();
            let count = (range.len() / PAGE_SIZE) as u64;
            let mut frames =
                parent.allocate(count, Rights::READ_WRITE | Rights::MAP | Rights::EXECUTE)?;
            frames.write(segment.memory.start - range.start, segment.data)?;
            let offset = (segment.offset - (segment.memory.start - range.start)) as u64;
            address_space.map_frames(
                Rc::new(frames),
                0,
                count,
                range.start,
                segment.permissions,
                PageBacking::File,
                Some(Rc::new(Origin::File(path))),
                offset,
            )?;
        }
        let stack = crate::stack::build(
            &address_space,
            &images.image,
            images.interpreter.as_ref().map_or(0, |image| image.base),
            arguments,
            random,
            stack,
        )?;
        Ok((address_space, stack))
    }

    /// Returns the number of bytes currently mapped into the address space.
    pub fn mapped(&self) -> usize {
        self.pages.borrow().len() * PAGE_SIZE
    }

    pub fn read(&self, address: usize, output: &mut [u8]) -> Result<(), runtime::Error> {
        self.native.task().read_memory(address, output)
    }

    pub fn write(&self, address: usize, input: &[u8]) -> Result<(), runtime::Error> {
        self.native.task().write_memory(address, input)
    }

    /// Atomically compares and exchanges one aligned user word.
    pub fn compare_exchange(
        &self,
        address: usize,
        expected: u32,
        new: u32,
    ) -> Result<Option<u32>, runtime::Error> {
        self.native
            .task()
            .compare_exchange_memory(address, expected, new)
    }

    /// Bytes covered by the current page records.
    pub fn mapped_bytes(&self) -> usize {
        self.pages.borrow().len() * PAGE_SIZE
    }

    /// Share the label used for the initial thread stack mapping.
    pub(super) fn stack_origin(&self) -> Rc<Origin> {
        Rc::clone(&self.stack_origin)
    }

    /// Contiguous mapped ranges with the permissions held by the native task.
    pub fn regions(&self) -> Result<Vec<Region>, runtime::Error> {
        let pages = self.pages.borrow();
        let mut regions: Vec<Region> = Vec::new();
        for (&address, page) in pages.iter() {
            let Some(info) = self.native.task().query(address)? else {
                continue;
            };
            let Some(permissions) = info.permissions else {
                continue;
            };
            if let Some(last) = regions.last_mut()
                && last.end == address
                && last.permissions == permissions
                && same_origin(last.origin.as_ref(), page.origin.as_ref())
                && page.file_offset
                    == if matches!(page.origin.as_deref(), Some(Origin::File(_))) {
                        last.file_offset + (address - last.start) as u64
                    } else {
                        0
                    }
            {
                last.end = address + PAGE_SIZE;
                continue;
            }
            regions.push(Region {
                start: address,
                end: address + PAGE_SIZE,
                permissions,
                origin: page.origin.clone(),
                file_offset: page.file_offset,
            });
        }
        Ok(regions)
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
            let pages = self.zeroed_pages(length, shared, None)?;
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
        self.map_zeroed(range.clone(), permissions, shared, None)?;
        Ok(range.start)
    }

    pub fn bounds(&self) -> Result<Range<usize>, runtime::Error> {
        self.native.task().address_range()
    }

    /// Collect every mapped page in `range`, failing if any page is not mapped.
    pub fn snapshot(&self, range: &Range<usize>) -> Result<Vec<MappedPage>, runtime::Error> {
        let mut pages = Vec::new();
        for address in range.clone().step_by(PAGE_SIZE) {
            let page = self
                .pages
                .borrow()
                .get(&address)
                .cloned()
                .ok_or(runtime::Error::Kernel(runtime::KernelError::BadAddress))?;
            let permissions = self
                .native
                .task()
                .query(address)?
                .and_then(|info| info.permissions)
                .ok_or(runtime::Error::Kernel(runtime::KernelError::BadAddress))?;
            pages.push(MappedPage {
                address,
                page,
                permissions,
            });
        }
        Ok(pages)
    }

    /// Remove the mapping covering `range` and return the pages that were mapped there.
    pub fn detach(&self, range: Range<usize>) -> Result<Vec<MappedPage>, runtime::Error> {
        let records: Vec<(usize, Page)> = self
            .pages
            .borrow()
            .range(range.clone())
            .map(|(address, page)| (*address, page.clone()))
            .collect();
        let mut saved = Vec::with_capacity(records.len());
        for (address, page) in records {
            let permissions = self
                .native
                .task()
                .query(address)?
                .and_then(|info| info.permissions)
                .ok_or(runtime::Error::Kernel(runtime::KernelError::Busy))?;
            saved.push(MappedPage {
                address,
                page,
                permissions,
            });
        }
        self.unmap(range)?;
        Ok(saved)
    }

    /// Re-establish pages returned by `snapshot` or `detach` at their recorded addresses.
    pub fn restore(&self, pages: &[MappedPage]) -> Result<(), runtime::Error> {
        for mapped in pages {
            let permissions = effective_permissions(&mapped.page, mapped.permissions);
            self.native.map(
                &mapped.page.frames,
                mapped.page.offset,
                1,
                Some(mapped.address),
                permissions,
            )?;
            self.pages
                .borrow_mut()
                .insert(mapped.address, mapped.page.clone());
        }
        Ok(())
    }

    /// Move `pages` to `start` and fill the remainder of `length` with zeroed pages.
    pub fn place(
        &self,
        pages: &[MappedPage],
        start: usize,
        length: usize,
        extension: Option<(Permissions, bool)>,
    ) -> Result<(), runtime::Error> {
        let placed_length = pages.len() * PAGE_SIZE;
        for (index, mapped) in pages.iter().enumerate() {
            let address = start + index * PAGE_SIZE;
            let permissions = effective_permissions(&mapped.page, mapped.permissions);
            if let Err(error) = self.native.map(
                &mapped.page.frames,
                mapped.page.offset,
                1,
                Some(address),
                permissions,
            ) {
                let _ = self.unmap(start..address);
                return Err(error);
            }
            self.pages.borrow_mut().insert(address, mapped.page.clone());
        }
        if length <= placed_length {
            return Ok(());
        }
        let Some((permissions, shared)) = extension else {
            let _ = self.unmap(start..start + placed_length);
            return Err(runtime::Error::Kernel(
                runtime::KernelError::InvalidArgument,
            ));
        };
        let origin = pages.first().and_then(|mapped| mapped.page.origin.clone());
        let prepared = match self.zeroed_pages(length - placed_length, shared, origin) {
            Ok(prepared) => prepared,
            Err(error) => {
                let _ = self.unmap(start..start + placed_length);
                return Err(error);
            }
        };
        if let Err(error) = self.install(start + placed_length, prepared, permissions) {
            let _ = self.unmap(start..start + placed_length);
            return Err(error);
        }
        Ok(())
    }

    /// Try to extend the mapping ending at `end` by `extra` zeroed pages.
    pub fn try_expand(
        &self,
        end: usize,
        extra: usize,
        permissions: Permissions,
        shared: bool,
    ) -> Result<bool, runtime::Error> {
        let mut reservation = match self.native.task().reserve(extra, PAGE_SIZE, Some(end)) {
            Ok(reservation) => reservation,
            Err(runtime::Error::Kernel(
                runtime::KernelError::Busy
                | runtime::KernelError::InvalidArgument
                | runtime::KernelError::NoMemory,
            )) => return Ok(false),
            Err(error) => return Err(error),
        };
        let Some(range) = reservation.range() else {
            return Err(runtime::Error::Protocol);
        };
        reservation.release()?;
        let origin = self
            .pages
            .borrow()
            .get(&(end - PAGE_SIZE))
            .and_then(|page| page.origin.clone());
        match self.map_zeroed(range, permissions, shared, origin) {
            Ok(()) => Ok(true),
            Err(runtime::Error::Kernel(
                runtime::KernelError::Busy
                | runtime::KernelError::InvalidArgument
                | runtime::KernelError::NoMemory,
            )) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Reserve any free gap of `length` bytes and return its start address.
    pub fn reserve_gap(&self, length: usize) -> Result<usize, runtime::Error> {
        let mut reservation = self.native.task().reserve(length, PAGE_SIZE, None)?;
        let Some(range) = reservation.range() else {
            return Err(runtime::Error::Protocol);
        };
        reservation.release()?;
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

    /// Reserves a huge mapping range, trying `address` first when it is a hint.
    ///
    /// The returned guard releases the range when dropped, so validation failures
    /// after this call cannot leak a reservation.
    pub(super) fn reserve_huge(
        &self,
        length: usize,
        address: Option<usize>,
    ) -> Result<Reservation, runtime::Error> {
        let range = match self.native.reserve(length, HUGE_PAGE_SIZE, address) {
            Err(runtime::Error::Kernel(
                runtime::KernelError::Busy | runtime::KernelError::InvalidArgument,
            )) if address.is_some() => self.native.reserve(length, HUGE_PAGE_SIZE, None),
            result => result,
        }?;
        Ok(Reservation::new(
            Rc::clone(&self.native),
            Rc::clone(&self.pending),
            range,
        ))
    }

    /// Maps `count` frames as 2 MiB large leaves and records the block at base page granularity.
    pub fn map_huge(
        &self,
        frames: Rc<Handle<FrameAllocation>>,
        offset: u64,
        count: u64,
        address: Option<usize>,
        permissions: Permissions,
        backing: PageBacking,
    ) -> Result<usize, runtime::Error> {
        if count == 0 || !count.is_multiple_of(HUGE_FRAMES) || !offset.is_multiple_of(HUGE_FRAMES) {
            return Err(runtime::Error::Kernel(
                runtime::KernelError::InvalidArgument,
            ));
        }
        if let Some(address) = address
            && !address.is_multiple_of(HUGE_PAGE_SIZE)
        {
            return Err(runtime::Error::Kernel(
                runtime::KernelError::InvalidArgument,
            ));
        }
        let range = self
            .native
            .map_huge(&frames, offset, count, address, permissions)?;
        let huge = Rc::clone(&frames);
        let mut pages = self.pages.borrow_mut();
        for index in 0..count {
            pages.insert(
                range.start + index as usize * PAGE_SIZE,
                Page {
                    frames: Rc::clone(&huge),
                    offset: offset + index,
                    backing,
                    huge: true,
                    origin: None,
                    file_offset: 0,
                },
            );
        }
        Ok(range.start)
    }

    /// Installs every materialized block of a huge mapping.
    ///
    /// All blocks are obtained before this call. A replacement removes the old
    /// range only after that point and restores it when a block cannot be installed,
    /// so a successful rollback leaves `MAP_FIXED` with the previous mapping intact.
    /// If unmapping the blocks installed so far or restoring the saved records
    /// fails, this returns `InvalidState` instead and the replaced range may be left
    /// only partially restored.
    pub(super) fn install_huge(
        &self,
        blocks: &[HugeBlock],
        replace: bool,
    ) -> Result<(), runtime::Error> {
        let first = blocks.first().ok_or(runtime::Error::Kernel(
            runtime::KernelError::InvalidArgument,
        ))?;
        let length = blocks
            .len()
            .checked_mul(HUGE_PAGE_SIZE)
            .ok_or(runtime::Error::Kernel(runtime::KernelError::NoMemory))?;
        let start = first.address;
        let end = start
            .checked_add(length)
            .ok_or(runtime::Error::Kernel(runtime::KernelError::NoMemory))?;
        let saved = if replace {
            let saved = self.save_pages(start..end)?;
            self.unmap(start..end)?;
            Some(saved)
        } else {
            None
        };
        for (mapped, block) in blocks.iter().enumerate() {
            if let Err(error) = self.map_huge(
                Rc::clone(&block.frames),
                0,
                HUGE_FRAMES,
                Some(block.address),
                block.permissions,
                block.backing,
            ) {
                let mut rollback_failed = false;
                for block in &blocks[..mapped] {
                    let address = block.address;
                    if self.unmap(address..address + HUGE_PAGE_SIZE).is_err() {
                        rollback_failed = true;
                    }
                }
                if let Some(saved) = saved
                    && self.restore_pages(saved).is_err()
                {
                    rollback_failed = true;
                }
                if rollback_failed {
                    return Err(runtime::Error::Kernel(runtime::KernelError::InvalidState));
                }
                return Err(error);
            }
        }
        Ok(())
    }

    /// Saves the mapping records in `range` with their effective permissions.
    ///
    /// A 2 MiB large leaf is one kernel mapping record, so the range is walked with
    /// one `query_next` per record instead of one query per base page.
    fn save_pages(&self, range: Range<usize>) -> Result<Vec<SavedPage>, runtime::Error> {
        let mut saved = Vec::new();
        saved
            .try_reserve(range.len() / PAGE_SIZE)
            .map_err(|_| runtime::Error::Kernel(runtime::KernelError::NoMemory))?;
        let mut cursor = range.start;
        while cursor < range.end {
            let Some(info) = self.native.task().query_next(cursor)? else {
                break;
            };
            if info.range.start >= range.end {
                break;
            }
            if info.range.start < cursor {
                // The range starts in the middle of a record, which the kernel
                // cannot remove as a whole.
                return Err(runtime::Error::Kernel(
                    runtime::KernelError::InvalidArgument,
                ));
            }
            if info.range.start > cursor {
                // Unmapped gap: the replacement only saves and restores records.
                cursor = info.range.start;
                continue;
            }
            let permissions = info.permissions.ok_or(runtime::Error::Kernel(
                runtime::KernelError::InvalidArgument,
            ))?;
            let page = self
                .pages
                .borrow()
                .get(&info.range.start)
                .cloned()
                .ok_or(runtime::Error::Protocol)?;
            saved.push(SavedPage {
                range: info.range.clone(),
                page,
                permissions,
            });
            cursor = info.range.end;
        }
        Ok(saved)
    }

    /// Restores saved mapping records, keeping large leaves as large leaves.
    fn restore_pages(&self, saved: Vec<SavedPage>) -> Result<(), runtime::Error> {
        let mut failed = false;
        for entry in saved {
            let effective = if matches!(entry.page.backing, PageBacking::BeyondEof(_)) {
                Permissions::NoAccess
            } else {
                entry.permissions
            };
            if entry.page.huge {
                if entry.range.len() != HUGE_PAGE_SIZE {
                    failed = true;
                    continue;
                }
                let count = (entry.range.len() / PAGE_SIZE) as u64;
                match self.native.map_huge(
                    &entry.page.frames,
                    entry.page.offset,
                    count,
                    Some(entry.range.start),
                    effective,
                ) {
                    Ok(_) => {
                        let mut pages = self.pages.borrow_mut();
                        for index in 0..count as usize {
                            let mut page = entry.page.clone();
                            page.offset += index as u64;
                            pages.insert(entry.range.start + index * PAGE_SIZE, page);
                        }
                    }
                    Err(_) => failed = true,
                }
                continue;
            }
            if entry.range.len() < PAGE_SIZE || !entry.range.len().is_multiple_of(PAGE_SIZE) {
                failed = true;
                continue;
            }
            // The kernel records one base page per mapping, so a saved record is
            // normally one page. Restore a wider record page by page rather than
            // losing the whole record if one page cannot be mapped.
            for index in 0..entry.range.len() / PAGE_SIZE {
                let address = entry.range.start + index * PAGE_SIZE;
                if self
                    .native
                    .map(
                        &entry.page.frames,
                        entry.page.offset + index as u64,
                        1,
                        Some(address),
                        effective,
                    )
                    .is_err()
                {
                    failed = true;
                    continue;
                }
                let mut page = entry.page.clone();
                page.offset += index as u64;
                self.pages.borrow_mut().insert(address, page);
            }
        }
        if failed {
            return Err(runtime::Error::Kernel(runtime::KernelError::InvalidState));
        }
        Ok(())
    }

    fn zeroed_pages(
        &self,
        length: usize,
        shared: bool,
        origin: Option<Rc<Origin>>,
    ) -> Result<Vec<Page>, runtime::Error> {
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
                origin: origin.clone(),
                file_offset: 0,
                huge: false,
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
        let saved = self.save_pages(range.clone())?;
        self.unmap(range)?;
        if let Err(error) = self.install(start, pages, permissions) {
            // Restore the replaced pages when installing the new mapping fails, so
            // the caller keeps the previous mapping. A failed restore is surfaced
            // instead of being hidden behind the install error.
            if self.restore_pages(saved).is_err() {
                return Err(runtime::Error::Kernel(runtime::KernelError::InvalidState));
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
            if matches!(page.backing, PageBacking::ReadOnlyFile)
                && permissions == Permissions::ReadWrite
            {
                return Err(runtime::Error::Kernel(runtime::KernelError::Denied));
            }
            if page.huge && (cursor != info.range.start || info.range.end > range.end) {
                return Err(runtime::Error::Kernel(
                    runtime::KernelError::InvalidArgument,
                ));
            }
            let end = if page.huge {
                info.range.end
            } else {
                cursor + PAGE_SIZE
            };
            changes.push((
                cursor..end,
                previous,
                if matches!(page.backing, PageBacking::BeyondEof(_)) {
                    Permissions::NoAccess
                } else {
                    permissions
                },
            ));
            cursor = end;
        }
        for (index, (address, _, next)) in changes.iter().enumerate() {
            if let Err(error) = self.native.protect(address.clone(), *next) {
                for (address, previous, _) in changes[..index].iter().rev() {
                    self.native.protect(address.clone(), *previous)?;
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
            if page.huge {
                return Err(runtime::Error::Kernel(runtime::KernelError::Unsupported));
            }
            if matches!(
                page.backing,
                PageBacking::File | PageBacking::ReadOnlyFile | PageBacking::BeyondEof(_)
            ) {
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
            if let Err(error) = self.map_frames(
                frames,
                0,
                1,
                address,
                permissions,
                PageBacking::Anonymous,
                None,
                0,
            ) {
                self.map_frames(
                    previous.frames,
                    previous.offset,
                    1,
                    address,
                    permissions,
                    previous.backing,
                    previous.origin,
                    previous.file_offset,
                )?;
                return Err(error);
            }
        }
        Ok(())
    }

    /// Checks that the range is mapped; shared huge page mappings need no write-back.
    pub fn msync(&self, range: Range<usize>) -> Result<(), runtime::Error> {
        let pages = self.pages.borrow();
        for address in range.step_by(PAGE_SIZE) {
            if !pages.contains_key(&address) {
                return Err(runtime::Error::Kernel(runtime::KernelError::BadAddress));
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
        // Large leaves cannot be split. Reject a range that only covers part of a
        // huge record before any earlier run is unmapped, so the address space is
        // unchanged when the call reports EINVAL.
        {
            let pages = self.pages.borrow();
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
                let huge = pages.get(&info.range.start).is_some_and(|page| page.huge);
                if huge && (info.range.start < range.start || info.range.end > range.end) {
                    return Err(runtime::Error::Kernel(
                        runtime::KernelError::InvalidArgument,
                    ));
                }
                cursor = info.range.end;
            }
        }
        let mut cursor = range.start;
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
    pub fn brk(&mut self, address: usize, limit: Option<usize>) -> Result<usize, runtime::Error> {
        if address < self.heap.start {
            return Ok(self.heap.end);
        }
        let Some(end) = address.checked_next_multiple_of(PAGE_SIZE) else {
            return Ok(self.heap.end);
        };
        let previous = self.heap.end.next_multiple_of(PAGE_SIZE);
        if let Some(limit) = limit
            && end > previous
            && self.mapped().saturating_add(end - previous) > limit
        {
            // A rejected growth keeps the previous break, like an allocation failure.
            return Ok(self.heap.end);
        }
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
        match self.map_zeroed(
            range,
            Permissions::ReadWrite,
            false,
            Some(Rc::clone(&self.heap_origin)),
        ) {
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
        origin: Option<Rc<Origin>>,
    ) -> Result<(), runtime::Error> {
        let prepared = self.zeroed_pages(range.len(), shared, origin)?;
        self.install(range.start, prepared, permissions)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn map_frames(
        &self,
        frames: Rc<Handle<FrameAllocation>>,
        offset: u64,
        count: u64,
        address: usize,
        permissions: Permissions,
        backing: PageBacking,
        origin: Option<Rc<Origin>>,
        file_offset: u64,
    ) -> Result<(), runtime::Error> {
        self.native
            .map(&frames, offset, count, Some(address), permissions)?;
        let file_backed = matches!(origin.as_deref(), Some(Origin::File(_)));
        let mut pages = self.pages.borrow_mut();
        for index in 0..count {
            pages.insert(
                address + index as usize * PAGE_SIZE,
                Page {
                    frames: Rc::clone(&frames),
                    offset: offset + index,
                    backing,
                    origin: origin.clone(),
                    file_offset: if file_backed {
                        file_offset + index * PAGE_SIZE as u64
                    } else {
                        0
                    },
                    huge: false,
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

fn same_origin(left: Option<&Rc<Origin>>, right: Option<&Rc<Origin>>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => Rc::ptr_eq(left, right),
        _ => false,
    }
}

impl AddressSpace {
    pub fn unmap_system_v(&self, range: Range<usize>, id: usize) -> Result<(), runtime::Error> {
        let addresses: Vec<usize> = self
            .pages
            .borrow()
            .range(range)
            .filter_map(|(address, page)| {
                matches!(page.backing, PageBacking::SystemV(segment) if segment == id)
                    .then_some(*address)
            })
            .collect();
        for address in addresses {
            self.unmap(address..address + PAGE_SIZE)?;
        }
        Ok(())
    }

    pub fn has_system_v(&self, range: Range<usize>, id: usize) -> bool {
        self.pages
            .borrow()
            .range(range)
            .any(|(_, page)| matches!(page.backing, PageBacking::SystemV(segment) if segment == id))
    }

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
        let key = if !private && let PageBacking::SystemV(id) = page.backing {
            (
                0,
                id * 2 + 1,
                page.offset as usize * PAGE_SIZE + address % PAGE_SIZE,
            )
        } else if private {
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
        self.clone_space(parent, port, false)
    }

    /// Create a second address space that maps the same frames without copying them.
    pub fn fork_shared(
        &self,
        parent: &Handle<Task>,
        port: &Handle<TrapPort>,
    ) -> Result<Self, runtime::Error> {
        self.clone_space(parent, port, true)
    }

    fn clone_space(
        &self,
        parent: &Handle<Task>,
        port: &Handle<TrapPort>,
        share: bool,
    ) -> Result<Self, runtime::Error> {
        let child = Self {
            native: Rc::new(SupervisedTask::new(parent, port)?),
            heap: self.heap.clone(),
            pages: RefCell::new(BTreeMap::new()),
            stack_origin: Rc::clone(&self.stack_origin),
            heap_origin: Rc::clone(&self.heap_origin),
            pending: Rc::new(RefCell::new(Vec::new())),
        };
        let bounds = self.native.task().address_range()?;
        // Copy the pending ranges instead of holding the `RefCell` borrow: the
        // query and reserve calls below can run a pending operation to completion,
        // whose guard removes its range from the same list.
        let pending = self.pending.borrow().clone();
        let mut cursor = bounds.start;
        while cursor < bounds.end {
            let Some(info) = self.native.task().query_next(cursor)? else {
                break;
            };
            cursor = info.range.end;
            // An in-flight huge mapping belongs to the parent's pending operation;
            // copying its reservation would leave the child with a range that only
            // the parent's guard can release.
            if info.permissions.is_none() && !pending.contains(&info.range) {
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
            if page.huge {
                if address.is_multiple_of(HUGE_PAGE_SIZE) {
                    // REFERENCE: https://man7.org/linux/man-pages/man2/fork.2.html
                    // File-backed and shared huge mappings stay aliased after fork.
                    // Only an anonymous private block is copied for the child.
                    let (frames, offset) =
                        if share || !matches!(page.backing, PageBacking::Anonymous) {
                            (Rc::clone(&page.frames), page.offset)
                        } else {
                            (copy_huge_block(parent, page)?, 0)
                        };
                    child.map_huge(
                        frames,
                        offset,
                        (HUGE_PAGE_SIZE / PAGE_SIZE) as u64,
                        Some(address),
                        permissions,
                        page.backing,
                    )?;
                }
                continue;
            }
            let (frames, offset) =
                if share || matches!(page.backing, PageBacking::Shared | PageBacking::SystemV(_)) {
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
            child.map_frames(
                frames,
                offset,
                1,
                address,
                permissions,
                page.backing,
                page.origin.clone(),
                page.file_offset,
            )?;
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
            None,
            0,
        )?;
        Ok(address)
    }
}

fn copy_huge_block(
    parent: &Handle<Task>,
    page: &Page,
) -> Result<Rc<Handle<FrameAllocation>>, runtime::Error> {
    const RIGHTS: Rights = Rights::READ_WRITE
        .union(Rights::MAP)
        .union(Rights::EXECUTE)
        .union(Rights::DUPLICATE)
        .union(Rights::TRANSFER);
    let mut frames = parent.allocate_contiguous(HUGE_FRAMES, HUGE_PAGE_SIZE, RIGHTS)?;
    let mut bytes = [0; PAGE_SIZE];
    for index in 0..HUGE_FRAMES {
        page.frames
            .read((page.offset + index) as usize * PAGE_SIZE, &mut bytes)?;
        frames.write(index as usize * PAGE_SIZE, &bytes)?;
    }
    Ok(Rc::new(frames))
}
