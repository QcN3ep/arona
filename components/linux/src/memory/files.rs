// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Mapped file identities and truncation updates across guest address spaces.

use super::{HUGE_PAGE_SIZE, PAGE_SIZE, Page, PageBacking};
use alloc::{
    collections::BTreeMap,
    rc::{Rc, Weak},
    vec::Vec,
};
use core::cell::{Cell, RefCell};
use runtime::{FrameAllocation, Handle, Rights, memory::Permissions, process::SupervisedTask};

pub(crate) struct File {
    pub source: fs::client::MappingSource,
    pub shared: bool,
    pub writable: bool,
    pub observed: Cell<(usize, u64)>,
}

#[derive(Clone)]
pub(crate) struct View {
    pub file: Rc<File>,
    pub generation: u64,
    pub size: usize,
}

type Pages = RefCell<BTreeMap<usize, Page>>;

#[derive(Default)]
pub(crate) struct Registry {
    spaces: RefCell<Vec<(Weak<SupervisedTask>, Weak<Pages>)>>,
    files: RefCell<Vec<Weak<File>>>,
}

impl Registry {
    pub fn register(&self, native: &Rc<SupervisedTask>) -> Rc<Pages> {
        let pages = Rc::new(RefCell::new(BTreeMap::new()));
        let mut spaces = self.spaces.borrow_mut();
        spaces.retain(|(native, _)| native.strong_count() != 0);
        spaces.push((Rc::downgrade(native), Rc::downgrade(&pages)));
        pages
    }

    pub fn track(&self, file: &Rc<File>) {
        let mut files = self.files.borrow_mut();
        files.retain(|entry| entry.strong_count() != 0);
        files.push(Rc::downgrade(file));
    }

    pub fn synchronize(&self) -> Result<(), runtime::Error> {
        let mut changes = Vec::new();
        for file in self.files.borrow().iter().filter_map(Weak::upgrade) {
            let state = file.source.state().map_err(error)?;
            if state != file.observed.get() {
                changes.push((file, state));
            }
        }
        if changes.is_empty() {
            return Ok(());
        }
        for (native, pages) in self.spaces.borrow().iter() {
            if let (Some(native), Some(pages)) = (native.upgrade(), pages.upgrade()) {
                synchronize(&native, &pages)?;
            }
        }
        for (file, state) in changes {
            file.observed.set(state);
        }
        Ok(())
    }
}

fn synchronize(native: &SupervisedTask, pages: &Pages) -> Result<(), runtime::Error> {
    let views: Vec<_> = pages
        .borrow()
        .iter()
        .filter_map(|(&address, page)| {
            if page.huge && !address.is_multiple_of(HUGE_PAGE_SIZE) {
                return None;
            }
            Some((address, page.clone(), page.file.clone()?))
        })
        .collect();
    for (address, page, mut view) in views {
        let (size, generation) = view.file.source.state().map_err(error)?;
        if size == view.size && generation == view.generation {
            continue;
        }
        let length = if page.huge { HUGE_PAGE_SIZE } else { PAGE_SIZE };
        let permissions = match page.backing {
            PageBacking::BeyondEof(permissions) => permissions,
            _ => native
                .task()
                .query(address)?
                .and_then(|info| info.permissions)
                .ok_or(runtime::Error::Protocol)?,
        };
        let mut backing = page.backing;
        let mut frames = Rc::clone(&page.frames);
        let mut offset = page.offset;
        if page.file_offset as usize >= size {
            native.protect(address..address + length, Permissions::NoAccess)?;
            backing = PageBacking::BeyondEof(permissions);
        } else if matches!(page.backing, PageBacking::BeyondEof(_))
            || view.file.shared && generation != view.generation
        {
            let source = view
                .file
                .source
                .map(page.file_offset as usize, view.file.writable)
                .map_err(error)?;
            frames = if view.file.shared {
                Rc::new(source)
            } else {
                copy(native, &source, length)?
            };
            offset = 0;
            backing = if view.file.shared && !view.file.writable {
                PageBacking::ReadOnlyFile
            } else {
                PageBacking::File
            };
            // Obtain the replacement before withdrawing the old mapping. The old
            // frames stay owned until the new view has been installed.
            native.unmap(address..address + length)?;
            let installed = if page.huge {
                native.map_huge(
                    &frames,
                    0,
                    (length / PAGE_SIZE) as u64,
                    Some(address),
                    permissions,
                )
            } else {
                native.map(&frames, 0, 1, Some(address), permissions)
            };
            if let Err(failure) = installed {
                if page.huge {
                    native.map_huge(
                        &page.frames,
                        page.offset,
                        (length / PAGE_SIZE) as u64,
                        Some(address),
                        Permissions::NoAccess,
                    )?;
                } else {
                    native.map(
                        &page.frames,
                        page.offset,
                        1,
                        Some(address),
                        Permissions::NoAccess,
                    )?;
                }
                return Err(failure);
            }
        }
        view.size = size;
        view.generation = generation;
        for index in 0..length / PAGE_SIZE {
            let mut next = page.clone();
            next.frames = Rc::clone(&frames);
            next.offset = offset + index as u64;
            next.file_offset = page.file_offset + (index * PAGE_SIZE) as u64;
            next.backing = backing;
            next.file = Some(view.clone());
            pages.borrow_mut().insert(address + index * PAGE_SIZE, next);
        }
    }
    Ok(())
}

pub(crate) fn copy(
    native: &SupervisedTask,
    source: &Handle<FrameAllocation>,
    length: usize,
) -> Result<Rc<Handle<FrameAllocation>>, runtime::Error> {
    let rights = Rights::READ_WRITE | Rights::MAP | Rights::EXECUTE;
    let mut frames = if length == HUGE_PAGE_SIZE {
        native
            .task()
            .allocate_contiguous((length / PAGE_SIZE) as u64, HUGE_PAGE_SIZE, rights)?
    } else {
        native.task().allocate(1, rights)?
    };
    let mut bytes = [0; PAGE_SIZE];
    for offset in (0..length).step_by(PAGE_SIZE) {
        source.read(offset, &mut bytes)?;
        frames.write(offset, &bytes)?;
    }
    Ok(Rc::new(frames))
}

fn error(error: fs::Error) -> runtime::Error {
    match error {
        fs::Error::Runtime(error) => error,
        fs::Error::NoSpace => runtime::Error::Kernel(runtime::KernelError::NoMemory),
        _ => runtime::Error::Protocol,
    }
}
