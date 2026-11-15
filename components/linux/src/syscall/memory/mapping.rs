// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Asynchronous probes, reads, and commits for file-backed mappings.
//!
//! A huge page file needs one file-service request per 2 MiB block. Materializing
//! those blocks synchronously inside the trap dispatcher would stall every other
//! guest and timer event for the whole mapping, so the mapping is a state machine:
//! first probe the descriptor, then request blocks one at a time, and install the
//! address space only after every block is available.

use super::{
    HUGE_PAGE_SIZE, HugeRequest, MAP_PRIVATE, O_WRONLY, PageBacking, allocate_huge,
    check_huge_file_mapping, huge_block_address, huge_error, map_error, mapping_error,
    validate_huge_request,
};
use crate::{
    elf::PAGE_SIZE,
    file::OpenFileDescription,
    memory::{HugeBlock, Origin, Page, Reservation},
    process::Process,
    syscall::{
        abi::{EACCES, EINVAL, EIO, ENOMEM, EOPNOTSUPP, EOVERFLOW, Errno, SyscallResult},
        file::{completion, filesystem_error},
    },
};
use alloc::{rc::Rc, vec::Vec};
use runtime::{Call, Handle, Rights, memory::Permissions};

pub(in crate::syscall) enum Pending<'a> {
    Complete(Option<SyscallResult>),
    File(alloc::boxed::Box<FileMapping<'a>>),
}

const MAPPING_TIMEOUT_NS: u64 = 10_000_000_000;

// TODO: Invalidate existing mappings after file truncation.
pub(in crate::syscall) struct FileMapping<'a> {
    origin: Option<Rc<Origin>>,
    description: Rc<OpenFileDescription<'a>>,
    request: Stage,
    /// Arguments of the original call, shared by every stage of the mapping.
    arguments: HugeRequest,
    /// Rounded length of an ordinary file mapping.
    length: usize,
    size: usize,
    pages: Vec<Page>,
    huge: Option<HugeMapping>,
    /// Deadline of the file-service request currently in flight.
    deadline: Option<u64>,
}

pub(super) enum Stage {
    /// Asynchronous huge page probe; also the first stage of ordinary mappings.
    Info(fs::client::PendingHugepageInfo),
    Metadata(fs::client::PendingMetadata),
    MapHuge(fs::client::PendingHugeMap),
    Read(fs::client::PendingRead),
    Prepare,
}

struct HugeMapping {
    start: usize,
    /// Address range held for a non-fixed mapping while its blocks are obtained.
    reservation: Option<Reservation>,
    size: u64,
    blocks: Vec<HugeBlock>,
    next: usize,
    count: usize,
}

impl<'a> FileMapping<'a> {
    pub(super) fn new(
        description: Rc<OpenFileDescription<'a>>,
        origin: Option<Rc<Origin>>,
        request: Stage,
        arguments: HugeRequest,
        length: usize,
    ) -> Self {
        Self {
            origin,
            description,
            request,
            arguments,
            length,
            size: 0,
            pages: Vec::new(),
            huge: None,
            deadline: mapping_deadline(),
        }
    }

    pub fn deadline(&self) -> Option<u64> {
        self.deadline
    }

    /// Bounds each file-service wait so a stalled service cannot pin a mapping.
    fn arm_deadline(&mut self) {
        self.deadline = mapping_deadline();
    }
}

impl Pending<'_> {
    pub fn deadline(&self) -> Option<u64> {
        match self {
            Self::Complete(_) => None,
            Self::File(mapping) => mapping.deadline(),
        }
    }

    pub fn call(&self) -> Option<&Handle<Call>> {
        match self {
            Self::Complete(_) => None,
            Self::File(mapping) => match &mapping.request {
                Stage::Info(request) => Some(request.call()),
                Stage::Metadata(request) => Some(request.call()),
                Stage::MapHuge(request) => Some(request.call()),
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
            Stage::Info(request) => {
                let hugepage = match completion(request.try_complete()) {
                    Some(Ok(size)) => size,
                    Some(Err(error)) => return self.fail(filesystem_error(error)),
                    None => return self.wait(),
                };
                if hugepage == 0 {
                    return self.start_ordinary();
                }
                if let Err(error) = self.begin_huge(process, hugepage) {
                    return Some(Err(error));
                }
                self.start_metadata()
            }
            Stage::Metadata(request) => {
                let metadata = match completion(request.try_complete()) {
                    Some(Ok(metadata)) => metadata,
                    Some(Err(error)) => return self.fail(filesystem_error(error)),
                    None => return self.wait(),
                };
                if self.huge.is_some() {
                    if !metadata.size.is_multiple_of(HUGE_PAGE_SIZE as u64) {
                        return self.fail(EINVAL);
                    }
                    if let Some(huge) = self.huge.as_mut() {
                        huge.size = metadata.size;
                    }
                    self.request = Stage::Prepare;
                    return self.advance_huge(process);
                }
                self.size = match usize::try_from(metadata.size) {
                    Ok(size) => size,
                    Err(_) => return self.fail(EOVERFLOW),
                };
                self.request = Stage::Prepare;
                self.advance_ordinary(process)
            }
            Stage::MapHuge(request) => {
                let frames = match completion(request.try_complete()) {
                    Some(Ok(frames)) => frames,
                    Some(Err(error)) => return self.fail(filesystem_error(error)),
                    None => return self.wait(),
                };
                let permissions = self.arguments.permissions;
                let Some(huge) = self.huge.as_mut() else {
                    return self.fail(EINVAL);
                };
                let address = match huge_block_address(huge.start, huge.next) {
                    Ok(address) => address,
                    Err(error) => return self.fail(error),
                };
                huge.blocks.push(HugeBlock {
                    address,
                    frames: Rc::new(frames),
                    permissions,
                    backing: PageBacking::File,
                });
                huge.next += 1;
                self.request = Stage::Prepare;
                self.advance_huge(process)
            }
            Stage::Read(request) => {
                let bytes = match completion(request.try_complete()) {
                    Some(Ok(bytes)) => bytes,
                    Some(Err(error)) => return self.fail(filesystem_error(error)),
                    None => return self.wait(),
                };
                let file_offset =
                    self.arguments.offset as u64 + self.pages.len() as u64 * PAGE_SIZE as u64;
                match page(process, &bytes, None, self.origin.clone(), file_offset) {
                    Ok(page) => self.pages.push(page),
                    Err(error) => return self.fail(error),
                }
                self.request = Stage::Prepare;
                self.advance_ordinary(process)
            }
            Stage::Prepare => {
                if self.huge.is_some() {
                    self.advance_huge(process)
                } else {
                    self.advance_ordinary(process)
                }
            }
        }
    }

    /// Reports a timeout only after the current request has been polled.
    ///
    /// A reply that is already queued must be consumed even when its deadline
    /// elapsed, so a completed file-service call is never reported as `EIO`.
    fn wait(&mut self) -> Option<SyscallResult> {
        let expired = self
            .deadline
            .is_some_and(|deadline| runtime::thread::now_ns().is_ok_and(|now| now >= deadline));
        if expired { self.fail(EIO) } else { None }
    }

    fn fail(&mut self, error: Errno) -> Option<SyscallResult> {
        let _ = self.release_reservation();
        Some(Err(error))
    }

    fn start_ordinary(&mut self) -> Option<SyscallResult> {
        if self.description.flags() & 3 == O_WRONLY {
            return Some(Err(EACCES));
        }
        if self.arguments.flags & 3 != MAP_PRIVATE {
            // TODO: Support MAP_SHARED for ordinary files; huge page files are mapped directly.
            return Some(Err(EOPNOTSUPP));
        }
        self.start_metadata()
    }

    fn start_metadata(&mut self) -> Option<SyscallResult> {
        let OpenFileDescription::Regular { file, .. } = self.description.as_ref() else {
            unreachable!()
        };
        match file.start_metadata() {
            Ok(request) => {
                self.request = Stage::Metadata(request);
                self.arm_deadline();
                None
            }
            Err(error) => self.fail(filesystem_error(error)),
        }
    }

    /// Validates a huge page file mapping and reserves its target range.
    ///
    /// A non-fixed mapping holds the reservation until installation, so another
    /// guest call cannot occupy the chosen gap while the blocks are fetched. The
    /// reservation guard releases the range on every later failure and again when
    /// the mapping is dropped.
    fn begin_huge(&mut self, process: &Process<'_>, hugepage: usize) -> Result<(), Errno> {
        if hugepage != HUGE_PAGE_SIZE {
            return Err(EINVAL);
        }
        let request = self.arguments;
        validate_huge_request(&request)?;
        check_huge_file_mapping(request.flags, request.permissions, &self.description)?;
        let (start, reservation) = super::huge_start(process, &request)?;
        self.huge = Some(HugeMapping {
            start,
            reservation,
            size: 0,
            blocks: Vec::new(),
            next: 0,
            count: request.length / HUGE_PAGE_SIZE,
        });
        Ok(())
    }

    fn advance_huge(&mut self, process: &Process<'_>) -> Option<SyscallResult> {
        loop {
            let Some(huge) = self.huge.as_ref() else {
                return self.fail(EINVAL);
            };
            let (start, next, count, size) = (huge.start, huge.next, huge.count, huge.size);
            let (offset, permissions) = (self.arguments.offset, self.arguments.permissions);
            if next == count {
                return Some(self.finish_huge(process));
            }
            let chunk_offset = match next
                .checked_mul(HUGE_PAGE_SIZE)
                .and_then(|chunk| chunk.checked_add(offset))
            {
                Some(chunk) => chunk,
                None => return self.fail(ENOMEM),
            };
            if (chunk_offset as u64) < size {
                let OpenFileDescription::Regular { file, .. } = self.description.as_ref() else {
                    return self.fail(EINVAL);
                };
                let request = match file.start_map_huge(chunk_offset) {
                    Ok(request) => request,
                    Err(error) => return self.fail(filesystem_error(error)),
                };
                self.request = Stage::MapHuge(request);
                self.arm_deadline();
                return None;
            }
            // The part beyond end of file is anonymous and needs no file request.
            let address = match huge_block_address(start, next) {
                Ok(address) => address,
                Err(error) => return self.fail(error),
            };
            let frames = match allocate_huge(process, super::HUGE_RIGHTS) {
                Ok(frames) => frames,
                Err(error) => return self.fail(error),
            };
            let huge = self.huge.as_mut()?;
            huge.blocks.push(HugeBlock {
                address,
                frames: Rc::new(frames),
                permissions: Permissions::NoAccess,
                backing: PageBacking::BeyondEof(permissions),
            });
            huge.next += 1;
        }
    }

    fn finish_huge(&mut self, process: &Process<'_>) -> SyscallResult {
        let Some(huge) = self.huge.as_ref() else {
            return Err(EINVAL);
        };
        let (start, replace, exact) = (huge.start, self.arguments.replace, self.arguments.exact);
        if let Err(error) = self.release_reservation() {
            // Do not publish the new mapping while the address space still reports
            // the range as reserved.
            return Err(huge_error(error, exact, replace));
        }
        let Some(huge) = self.huge.as_ref() else {
            return Err(EINVAL);
        };
        process
            .address_space
            .install_huge(&huge.blocks, replace)
            .map_err(|error| huge_error(error, exact, replace))?;
        Ok(start)
    }

    fn release_reservation(&mut self) -> Result<(), runtime::Error> {
        match self
            .huge
            .as_mut()
            .and_then(|huge| huge.reservation.as_mut())
        {
            Some(reservation) => reservation.release(),
            None => Ok(()),
        }
    }

    fn advance_ordinary(&mut self, process: &Process<'_>) -> Option<SyscallResult> {
        while self.pages.len() < self.length / PAGE_SIZE {
            let offset = self.arguments.offset + self.pages.len() * PAGE_SIZE;
            if offset < self.size {
                let OpenFileDescription::Regular { file, .. } = self.description.as_ref() else {
                    unreachable!()
                };
                return match file.start_read_at(offset, PAGE_SIZE.min(self.size - offset)) {
                    Ok(request) => {
                        self.request = Stage::Read(request);
                        self.arm_deadline();
                        None
                    }
                    Err(error) => Some(Err(filesystem_error(error))),
                };
            }
            let file_offset =
                self.arguments.offset as u64 + self.pages.len() as u64 * PAGE_SIZE as u64;
            match page(
                process,
                &[],
                Some(self.arguments.permissions),
                self.origin.clone(),
                file_offset,
            ) {
                Ok(page) => self.pages.push(page),
                Err(error) => return Some(Err(error)),
            }
        }
        Some(
            process
                .address_space
                .map_prepared(
                    (self.arguments.address != 0)
                        .then_some(self.arguments.address & !(PAGE_SIZE - 1)),
                    core::mem::take(&mut self.pages),
                    self.arguments.permissions,
                    self.arguments.exact,
                    self.arguments.replace,
                )
                .map_err(|error| {
                    if self.arguments.replace {
                        mapping_error(error)
                    } else {
                        map_error(error, self.arguments.exact)
                    }
                }),
        )
    }
}

fn mapping_deadline() -> Option<u64> {
    runtime::thread::now_ns()
        .ok()?
        .checked_add(MAPPING_TIMEOUT_NS)
}

fn page(
    process: &Process<'_>,
    bytes: &[u8],
    bus: Option<Permissions>,
    origin: Option<Rc<Origin>>,
    file_offset: u64,
) -> Result<Page, Errno> {
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
        origin,
        file_offset,
        huge: false,
    })
}
