// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Asynchronous reads and commits for private file mappings.

use super::{map_error, mapping_error};
use crate::{
    elf::PAGE_SIZE,
    file::OpenFileDescription,
    memory::{Page, PageBacking},
    process::Process,
    syscall::{
        abi::{EOVERFLOW, Errno, SyscallResult},
        file::{completion, filesystem_error},
    },
};
use alloc::{rc::Rc, vec::Vec};
use runtime::{Call, Handle, Rights, memory::Permissions};

pub(in crate::syscall) enum Pending<'a> {
    Complete(Option<SyscallResult>),
    File(FileMapping<'a>),
}

// TODO: Invalidate existing mappings after file truncation.
pub(in crate::syscall) struct FileMapping<'a> {
    pub(super) description: Rc<OpenFileDescription<'a>>,
    pub(super) request: Stage,
    pub(super) address: Option<usize>,
    pub(super) length: usize,
    pub(super) offset: usize,
    pub(super) permissions: Permissions,
    pub(super) exact: bool,
    pub(super) replace: bool,
    pub(super) size: usize,
    pub(super) pages: Vec<Page>,
}

pub(super) enum Stage {
    Metadata(fs::client::PendingMetadata),
    Read(fs::client::PendingRead),
    Prepare,
}

impl Pending<'_> {
    pub fn call(&self) -> Option<&Handle<Call>> {
        match self {
            Self::Complete(_) => None,
            Self::File(mapping) => match &mapping.request {
                Stage::Metadata(request) => Some(request.call()),
                Stage::Read(request) => Some(request.call()),
                Stage::Prepare => None,
            },
        }
    }

    pub fn poll(&mut self, process: &Process<'_>) -> Option<SyscallResult> {
        match self {
            Self::Complete(result) => result.take(),
            Self::File(mapping) => mapping.poll(process),
        }
    }
}

impl FileMapping<'_> {
    fn poll(&mut self, process: &Process<'_>) -> Option<SyscallResult> {
        match &mut self.request {
            Stage::Metadata(request) => {
                let metadata = match completion(request.try_complete())? {
                    Ok(metadata) => metadata,
                    Err(error) => return Some(Err(filesystem_error(error))),
                };
                self.size = match usize::try_from(metadata.size) {
                    Ok(size) => size,
                    Err(_) => return Some(Err(EOVERFLOW)),
                };
            }
            Stage::Read(request) => {
                let bytes = match completion(request.try_complete())? {
                    Ok(bytes) => bytes,
                    Err(error) => return Some(Err(filesystem_error(error))),
                };
                match page(process, &bytes, None) {
                    Ok(page) => self.pages.push(page),
                    Err(error) => return Some(Err(error)),
                }
            }
            Stage::Prepare => {}
        }
        self.request = Stage::Prepare;
        while self.pages.len() < self.length / PAGE_SIZE {
            let offset = self.offset + self.pages.len() * PAGE_SIZE;
            if offset < self.size {
                let OpenFileDescription::Regular { file, .. } = self.description.as_ref() else {
                    unreachable!()
                };
                return match file.start_read_at(offset, PAGE_SIZE.min(self.size - offset)) {
                    Ok(request) => {
                        self.request = Stage::Read(request);
                        None
                    }
                    Err(error) => Some(Err(filesystem_error(error))),
                };
            }
            match page(process, &[], Some(self.permissions)) {
                Ok(page) => self.pages.push(page),
                Err(error) => return Some(Err(error)),
            }
        }
        Some(
            process
                .address_space
                .map_prepared(
                    self.address,
                    core::mem::take(&mut self.pages),
                    self.permissions,
                    self.exact,
                    self.replace,
                )
                .map_err(|error| {
                    if self.replace {
                        mapping_error(error)
                    } else {
                        map_error(error, self.exact)
                    }
                }),
        )
    }
}

fn page(process: &Process<'_>, bytes: &[u8], bus: Option<Permissions>) -> Result<Page, Errno> {
    let mut frames = process
        .address_space
        .native
        .task()
        .allocate(1, Rights::READ_WRITE | Rights::MAP | Rights::EXECUTE)
        .map_err(mapping_error)?;
    frames.write(0, bytes).map_err(mapping_error)?;
    Ok(Page {
        frames: Rc::new(frames),
        offset: 0,
        backing: bus.map_or(PageBacking::File, PageBacking::BeyondEof),
    })
}
