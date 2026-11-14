// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use core::ops::Range;
use runtime::memory::Permissions;

use super::file::filesystem_error;
use super::{
    Outcome,
    abi::{
        EACCES, EBADF, EEXIST, EFAULT, EINVAL, EIO, ENODEV, ENOMEM, EOPNOTSUPP, EOVERFLOW, Errno,
        SyscallResult,
    },
};
use crate::{
    elf::PAGE_SIZE,
    file::OpenFileDescription,
    memory::{AddressSpace, MappedPage, Origin, PageBacking},
    process::Process,
};
use alloc::{rc::Rc, vec::Vec};

pub(super) use mapping::Pending;
use mapping::{FileMapping, Stage};
mod mapping;

// REFERENCE: https://man7.org/linux/man-pages/man2/mmap.2.html
const MAP_PRIVATE: usize = 2;
const MAP_FIXED: usize = 0x10;
const MAP_ANONYMOUS: usize = 0x20;
const MAP_FIXED_NOREPLACE: usize = 0x100000;

// REFERENCE: https://man7.org/linux/man-pages/man2/msync.2.html
const MS_ASYNC: usize = 1;
const MS_INVALIDATE: usize = 2;
const MS_SYNC: usize = 4;

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
    if description.path_only() {
        return Err(EBADF);
    }
    let OpenFileDescription::Regular { file, path, .. } = description.as_ref() else {
        return Err(ENODEV);
    };
    if description.flags() & 3 == 1 {
        return Err(EACCES);
    }
    offset
        .checked_add(length)
        .filter(|end| *end <= i64::MAX as usize)
        .ok_or(EOVERFLOW)?;
    let origin = path
        .as_ref()
        .map(|path| Rc::new(Origin::File(Rc::clone(path))));
    let request = Stage::Metadata(file.start_metadata().map_err(filesystem_error)?);
    Ok(Pending::File(FileMapping {
        description,
        origin,
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

pub(super) fn msync(
    space: &AddressSpace,
    address: usize,
    length: usize,
    flags: usize,
) -> SyscallResult {
    if flags & !(MS_ASYNC | MS_INVALIDATE | MS_SYNC) != 0
        || flags & MS_ASYNC != 0 && flags & MS_SYNC != 0
    {
        return Err(EINVAL);
    }
    let range = range(address, length)?;
    space.msync(range).map_err(mapping_error)?;
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

const MREMAP_MAYMOVE: usize = 1;
const MREMAP_FIXED: usize = 2;
const MREMAP_DONTUNMAP: usize = 4;

// REFERENCE: https://man7.org/linux/man-pages/man2/mremap.2.html
pub(super) fn mremap(space: &AddressSpace, arguments: [usize; 6]) -> SyscallResult {
    let [old_address, old_size, new_size, flags, new_address, ..] = arguments;
    if flags & !(MREMAP_MAYMOVE | MREMAP_FIXED | MREMAP_DONTUNMAP) != 0
        || flags & MREMAP_FIXED != 0 && flags & MREMAP_MAYMOVE == 0
        || flags & MREMAP_DONTUNMAP != 0 && (flags & MREMAP_MAYMOVE == 0 || old_size != new_size)
        || old_size == 0
        || new_size == 0
        || !old_address.is_multiple_of(PAGE_SIZE)
    {
        return Err(EINVAL);
    }
    if flags & MREMAP_DONTUNMAP != 0 {
        return Err(EOPNOTSUPP);
    }
    let old = range(old_address, old_size)?;
    let new_length = new_size
        .checked_next_multiple_of(PAGE_SIZE)
        .filter(|length| *length <= isize::MAX as usize)
        .ok_or(ENOMEM)?;
    let bounds = space.bounds().map_err(mapping_error)?;
    if old.start < bounds.start || old.end > bounds.end {
        return Err(EFAULT);
    }
    let pages = space.snapshot(&old).map_err(old_error)?;
    let extension = if new_length > old.len() {
        Some(extension(&pages)?)
    } else {
        None
    };
    if flags & MREMAP_FIXED != 0 {
        return move_fixed(space, old, pages, new_length, new_address, extension);
    }
    if new_length <= old.len() {
        if new_length < old.len() {
            space
                .unmap(old.start + new_length..old.end)
                .map_err(mapping_error)?;
        }
        return Ok(old_address);
    }
    let Some(extension) = extension else {
        return Err(ENOMEM);
    };
    if space
        .try_expand(old.end, new_length - old.len(), extension.0, extension.1)
        .map_err(mapping_error)?
    {
        return Ok(old_address);
    }
    if flags & MREMAP_MAYMOVE == 0 {
        return Err(ENOMEM);
    }
    grow(space, old, &pages, new_length, extension)
}

fn old_error(error: runtime::Error) -> Errno {
    match error {
        runtime::Error::Kernel(
            runtime::KernelError::BadAddress
            | runtime::KernelError::Busy
            | runtime::KernelError::InvalidArgument,
        ) => EFAULT,
        error => mapping_error(error),
    }
}

/// Describe how a mapping may grow; file mappings cannot gain pages without file reads.
fn extension(pages: &[MappedPage]) -> Result<(Permissions, bool), Errno> {
    let first = pages.first().ok_or(EINVAL)?;
    if matches!(
        first.page.backing,
        PageBacking::File | PageBacking::BeyondEof(_)
    ) {
        return Err(ENOMEM);
    }
    let shared = matches!(first.page.backing, PageBacking::Shared);
    for mapped in &pages[1..] {
        if mapped.permissions != first.permissions
            || matches!(
                mapped.page.backing,
                PageBacking::File | PageBacking::BeyondEof(_)
            )
            || matches!(mapped.page.backing, PageBacking::Shared) != shared
        {
            return Err(EFAULT);
        }
    }
    Ok((first.permissions, shared))
}

fn move_fixed(
    space: &AddressSpace,
    old: Range<usize>,
    pages: Vec<MappedPage>,
    new_length: usize,
    new_address: usize,
    extension: Option<(Permissions, bool)>,
) -> SyscallResult {
    if new_address == 0 || !new_address.is_multiple_of(PAGE_SIZE) {
        return Err(EINVAL);
    }
    let end = new_address.checked_add(new_length).ok_or(EINVAL)?;
    let bounds = space.bounds().map_err(mapping_error)?;
    if new_address < bounds.start || end > bounds.end || old.start < end && new_address < old.end {
        return Err(EINVAL);
    }
    let destination = new_address..end;
    let saved = space
        .detach(destination.clone())
        .map_err(|error| match error {
            runtime::Error::Kernel(
                runtime::KernelError::Busy | runtime::KernelError::InvalidArgument,
            ) => EINVAL,
            error => mapping_error(error),
        })?;
    let moved = new_length.min(old.len()) / PAGE_SIZE;
    let result = space
        .place(&pages[..moved], new_address, new_length, extension)
        .and_then(|()| space.unmap(old));
    match result {
        Ok(()) => Ok(new_address),
        Err(error) => {
            let _ = space.unmap(destination);
            let _ = space.restore(&saved);
            Err(mapping_error(error))
        }
    }
}

fn grow(
    space: &AddressSpace,
    old: Range<usize>,
    pages: &[MappedPage],
    new_length: usize,
    extension: (Permissions, bool),
) -> SyscallResult {
    let destination = space.reserve_gap(new_length).map_err(mapping_error)?;
    space
        .place(pages, destination, new_length, Some(extension))
        .map_err(mapping_error)?;
    if let Err(error) = space.unmap(old) {
        let _ = space.unmap(destination..destination + new_length);
        return Err(mapping_error(error));
    }
    Ok(destination)
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
