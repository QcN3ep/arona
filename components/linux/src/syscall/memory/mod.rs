// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

// TODO: Support msync and mremap.

use core::ops::Range;
use runtime::memory::Permissions;

use super::file::filesystem_error;
use super::{
    Outcome,
    abi::{
        EACCES, EBADF, EEXIST, EINVAL, EIO, ENODEV, ENOMEM, EOPNOTSUPP, EOVERFLOW, Errno,
        SyscallResult,
    },
};
use crate::{elf::PAGE_SIZE, file::OpenFileDescription, memory::AddressSpace, process::Process};
use alloc::{rc::Rc, vec::Vec};

pub(super) use mapping::Pending;
use mapping::{FileMapping, Stage};
mod mapping;

// REFERENCE: https://man7.org/linux/man-pages/man2/mmap.2.html
const MAP_PRIVATE: usize = 2;
const MAP_FIXED: usize = 0x10;
const MAP_ANONYMOUS: usize = 0x20;
const MAP_FIXED_NOREPLACE: usize = 0x100000;

pub(super) fn mmap<'a>(process: &Process<'a>, arguments: [usize; 6]) -> Outcome<'a> {
    match start(process, arguments) {
        Ok(operation) => Outcome::Pending(super::Operation::Memory(operation)),
        Err(error) => Outcome::Complete(Err(error)),
    }
}

fn start<'a>(process: &Process<'a>, arguments: [usize; 6]) -> Result<Pending<'a>, Errno> {
    let [address, length, protection, flags, raw, offset] = arguments;
    if length == 0 || !offset.is_multiple_of(PAGE_SIZE) || !matches!(flags & 3, 1 | 2) {
        return Err(EINVAL);
    }
    let permissions = permissions(protection)?;
    if flags & !(3 | MAP_ANONYMOUS | MAP_FIXED | MAP_FIXED_NOREPLACE) != 0
        || flags & MAP_FIXED != 0 && flags & MAP_FIXED_NOREPLACE != 0
    {
        return Err(EINVAL);
    }
    let length = length
        .checked_next_multiple_of(PAGE_SIZE)
        .filter(|length| *length <= isize::MAX as usize)
        .ok_or(ENOMEM)?;
    let exact = flags & (MAP_FIXED | MAP_FIXED_NOREPLACE) != 0;
    let replace = flags & MAP_FIXED != 0;
    if exact && !address.is_multiple_of(PAGE_SIZE) {
        return Err(EINVAL);
    }
    if exact && address == 0 {
        return Err(ENOMEM);
    }
    if flags & MAP_ANONYMOUS != 0 {
        let result = process
            .address_space
            .map_anonymous(
                (address != 0).then_some(address & !(PAGE_SIZE - 1)),
                length,
                permissions,
                exact,
                replace,
                flags & 3 == 1,
            )
            .map_err(|error| {
                if replace {
                    mapping_error(error)
                } else {
                    map_error(error, exact)
                }
            });
        return Ok(Pending::Complete(Some(result)));
    }
    if flags & 3 != MAP_PRIVATE {
        // TODO: Support file MAP_SHARED and synchronize data with the file service.
        return Err(EOPNOTSUPP);
    }
    let description = Rc::clone(
        &process
            .files
            .get(raw as i32 as usize)
            .ok_or(EBADF)?
            .description,
    );
    let OpenFileDescription::Regular { file, .. } = description.as_ref() else {
        return Err(ENODEV);
    };
    if description.flags() & 3 == 1 {
        return Err(EACCES);
    }
    offset
        .checked_add(length)
        .filter(|end| *end <= i64::MAX as usize)
        .ok_or(EOVERFLOW)?;
    let request = Stage::Metadata(file.start_metadata().map_err(filesystem_error)?);
    Ok(Pending::File(FileMapping {
        description,
        request,
        address: (address != 0).then_some(address & !(PAGE_SIZE - 1)),
        length,
        offset,
        permissions,
        exact,
        replace,
        size: 0,
        pages: Vec::new(),
    }))
}

fn map_error(error: runtime::Error, exact: bool) -> Errno {
    match error {
        runtime::Error::Kernel(runtime::KernelError::Busy) if exact => EEXIST,
        error => mapping_error(error),
    }
}

pub(super) fn munmap(space: &AddressSpace, address: usize, length: usize) -> SyscallResult {
    if length == 0 {
        return Err(EINVAL);
    }
    let range = range(address, length).map_err(|_| EINVAL)?;
    space.unmap(range).map_err(|error| match error {
        runtime::Error::Kernel(
            runtime::KernelError::Busy | runtime::KernelError::InvalidArgument,
        ) => EINVAL,
        error => mapping_error(error),
    })?;
    Ok(0)
}

// REFERENCE: https://man7.org/linux/man-pages/man2/mprotect.2.html
pub(super) fn mprotect(
    space: &AddressSpace,
    address: usize,
    length: usize,
    protection: usize,
) -> SyscallResult {
    let permissions = permissions(protection)?;
    let range = range(address, length)?;
    if !range.is_empty() {
        space.protect(range, permissions).map_err(mapping_error)?;
    }
    Ok(0)
}

fn range(address: usize, length: usize) -> Result<Range<usize>, Errno> {
    if !address.is_multiple_of(PAGE_SIZE) {
        return Err(EINVAL);
    }
    let end = length
        .checked_next_multiple_of(PAGE_SIZE)
        .and_then(|length| address.checked_add(length))
        .ok_or(ENOMEM)?;
    Ok(address..end)
}

fn permissions(protection: usize) -> Result<Permissions, Errno> {
    match protection {
        0 => Ok(Permissions::NoAccess),
        1 => Ok(Permissions::ReadOnly),
        2 | 3 => Ok(Permissions::ReadWrite),
        4 => Ok(Permissions::ExecuteOnly),
        5 => Ok(Permissions::ReadExecute),
        6 | 7 => Err(EACCES),
        _ => Err(EINVAL),
    }
}

fn mapping_error(error: runtime::Error) -> Errno {
    match error {
        runtime::Error::Kernel(runtime::KernelError::Denied | runtime::KernelError::Busy) => EACCES,
        runtime::Error::Kernel(
            runtime::KernelError::NoMemory
            | runtime::KernelError::BadAddress
            | runtime::KernelError::InvalidArgument,
        ) => ENOMEM,
        _ => EIO,
    }
}

pub(super) fn madvise(space: &AddressSpace, arguments: [usize; 6]) -> SyscallResult {
    let [address, length, advice, ..] = arguments;
    if advice as i32 != 4 {
        return Err(EOPNOTSUPP);
    }
    let range = range(address, length)?;
    space.discard(range).map_err(|error| match error {
        runtime::Error::Kernel(runtime::KernelError::Unsupported) => EOPNOTSUPP,
        error => mapping_error(error),
    })?;
    Ok(0)
}
