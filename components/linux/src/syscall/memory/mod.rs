// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use core::ops::Range;
use runtime::memory::{Permissions, PermissionsExt};

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
    memory::{AddressSpace, HugeBlock, MappedPage, Origin, PageBacking, Reservation},
    process::Process,
};
use alloc::{rc::Rc, vec::Vec};
use runtime::{FrameAllocation, Handle, Rights};

pub(super) use mapping::Pending;
use mapping::{FileMapping, Stage};
mod mapping;

// REFERENCE: https://man7.org/linux/man-pages/man2/mmap.2.html
const O_RDONLY: u32 = 0;
const O_WRONLY: u32 = 1;
const MAP_SHARED: usize = 1;
const MAP_PRIVATE: usize = 2;
const MAP_FIXED: usize = 0x10;
const MAP_ANONYMOUS: usize = 0x20;
// Linux accepts MAP_STACK without changing the mapping behavior.
const MAP_STACK: usize = 0x20000;
const MAP_FIXED_NOREPLACE: usize = 0x100000;
// REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/include/uapi/asm-generic/mman-common.h?h=v6.12
const MAP_HUGETLB: usize = 0x40000;
// REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/include/uapi/asm-generic/hugetlb_encode.h?h=v6.12
// REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/include/uapi/linux/mman.h?h=v6.12
const MAP_HUGE_SHIFT: u32 = 26;
const MAP_HUGE_MASK: usize = 0x3f << MAP_HUGE_SHIFT;
const MAP_HUGE_2MB: usize = 21 << MAP_HUGE_SHIFT;
const HUGE_PAGE_SIZE: usize = 2 * 1024 * 1024;
const HUGE_FRAMES: u64 = (HUGE_PAGE_SIZE / PAGE_SIZE) as u64;
const HUGE_RIGHTS: Rights = Rights::READ_WRITE
    .union(Rights::MAP)
    .union(Rights::EXECUTE)
    .union(Rights::DUPLICATE)
    .union(Rights::TRANSFER);

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
    if length == 0 || !matches!(flags & 3, MAP_SHARED | MAP_PRIVATE) {
        return Err(EINVAL);
    }
    let permissions = permissions(protection)?;
    if flags
        & !(3
            | MAP_ANONYMOUS
            | MAP_FIXED
            | MAP_STACK
            | MAP_FIXED_NOREPLACE
            | MAP_HUGETLB
            | MAP_HUGE_MASK)
        != 0
        || flags & MAP_FIXED != 0 && flags & MAP_FIXED_NOREPLACE != 0
    {
        return Err(EINVAL);
    }
    let huge = flags & MAP_HUGETLB != 0;
    let encoding = flags & MAP_HUGE_MASK;
    if encoding != 0 && (!huge || encoding != MAP_HUGE_2MB) {
        return Err(EINVAL);
    }
    let exact = flags & (MAP_FIXED | MAP_FIXED_NOREPLACE) != 0;
    let replace = flags & MAP_FIXED != 0;
    let anonymous = flags & MAP_ANONYMOUS != 0;
    if exact && address == 0 {
        return Err(ENOMEM);
    }
    if huge && anonymous {
        return Ok(Pending::Complete(Some(anonymous_huge_mapping(
            process,
            HugeRequest {
                address,
                length,
                offset,
                flags,
                permissions,
                exact,
                replace,
            },
        ))));
    }
    if !offset.is_multiple_of(PAGE_SIZE) {
        return Err(EINVAL);
    }
    // The file probe determines the leaf size before rounding its request.
    let requested_length = length;
    let length = length
        .checked_next_multiple_of(PAGE_SIZE)
        .filter(|length| *length <= isize::MAX as usize)
        .ok_or(ENOMEM)?;
    if process
        .limits
        .address_space()
        .is_some_and(|limit| process.address_space.mapped().saturating_add(length) > limit)
    {
        return Err(ENOMEM);
    }
    if exact
        && address != 0
        && process.attaches.iter().any(|(start, attach)| {
            *start < address.saturating_add(length)
                && start.saturating_add(attach.mapped_length) > address
        })
    {
        // A fixed replacement would detach a System V mapping without updating
        // its attachment record, so keep the mapping owned by shmdt.
        return Err(EINVAL);
    }
    if exact && !address.is_multiple_of(PAGE_SIZE) {
        return Err(EINVAL);
    }
    if anonymous {
        let result = process
            .address_space
            .map_anonymous(
                (address != 0).then_some(address & !(PAGE_SIZE - 1)),
                length,
                permissions,
                exact,
                replace,
                flags & 3 == MAP_SHARED,
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
    offset
        .checked_add(length)
        .filter(|end| *end <= i64::MAX as usize)
        .ok_or(EOVERFLOW)?;
    let origin = path
        .as_ref()
        .map(|path| Rc::new(Origin::File(Rc::clone(path))));
    // Probe the descriptor asynchronously so a huge page file does not make the
    // caller wait for the file service inside the trap dispatch loop. Ordinary
    // files use the same probe and continue with their metadata request.
    let request = Stage::Info(file.start_hugepage_info().map_err(filesystem_error)?);
    Ok(Pending::File(alloc::boxed::Box::new(FileMapping::new(
        description,
        origin,
        request,
        HugeRequest {
            address,
            length: requested_length,
            offset,
            flags,
            permissions,
            exact,
            replace,
        },
        length,
    ))))
}

#[derive(Clone, Copy)]
pub(super) struct HugeRequest {
    address: usize,
    length: usize,
    offset: usize,
    flags: usize,
    permissions: Permissions,
    exact: bool,
    replace: bool,
}

pub(super) fn normalize_huge_request(mut request: HugeRequest) -> Result<HugeRequest, Errno> {
    request.length = request
        .length
        .checked_next_multiple_of(HUGE_PAGE_SIZE)
        .ok_or(ENOMEM)?;
    if !request.exact {
        request.address &= !(HUGE_PAGE_SIZE - 1);
    }
    Ok(request)
}

/// Validates the flags and offsets shared by anonymous and file-backed huge mappings.
pub(super) fn validate_huge_request(request: &HugeRequest) -> Result<(), Errno> {
    if !request.length.is_multiple_of(HUGE_PAGE_SIZE)
        || !request.offset.is_multiple_of(HUGE_PAGE_SIZE)
        || request.address != 0 && !request.address.is_multiple_of(HUGE_PAGE_SIZE)
    {
        return Err(EINVAL);
    }
    if request.length > isize::MAX as usize {
        return Err(ENOMEM);
    }
    if request
        .offset
        .checked_add(request.length)
        .filter(|end| *end <= i64::MAX as usize)
        .is_none()
    {
        return Err(EOVERFLOW);
    }
    if request.exact && !request.address.is_multiple_of(HUGE_PAGE_SIZE) {
        return Err(EINVAL);
    }
    if request.address != 0 && !request.exact && !request.address.is_multiple_of(PAGE_SIZE) {
        return Err(EINVAL);
    }
    Ok(())
}

fn anonymous_huge_mapping(process: &Process<'_>, request: HugeRequest) -> SyscallResult {
    let request = normalize_huge_request(request)?;
    validate_huge_request(&request)?;
    if process.limits.address_space().is_some_and(|limit| {
        process
            .address_space
            .mapped()
            .saturating_add(request.length)
            > limit
    }) {
        return Err(ENOMEM);
    }
    let start = reserve_huge_start(process, &request)?;
    let count = request.length / HUGE_PAGE_SIZE;
    let backing = if request.flags & 3 == MAP_SHARED {
        PageBacking::Shared
    } else {
        PageBacking::Anonymous
    };
    let mut blocks = Vec::new();
    blocks.try_reserve(count).map_err(|_| ENOMEM)?;
    for index in 0..count {
        blocks.push(HugeBlock {
            address: huge_block_address(start, index)?,
            frames: Rc::new(allocate_huge(process, HUGE_RIGHTS)?),
            permissions: request.permissions,
            backing,
        });
    }
    process
        .address_space
        .install_huge(&blocks, request.replace)
        .map_err(|error| huge_error(error, request.exact, request.replace))?;
    Ok(start)
}

fn reserve_huge_start(process: &Process<'_>, request: &HugeRequest) -> Result<usize, Errno> {
    let (start, reservation) = huge_start(process, request)?;
    if let Some(mut reservation) = reservation {
        reservation
            .release()
            .map_err(|error| huge_error(error, request.exact, request.replace))?;
    }
    Ok(start)
}

/// Chooses and validates the start of a huge mapping.
///
/// A non-fixed mapping keeps its reservation in the returned guard, so validation
/// failures release the range automatically. Fixed mappings never reserve.
fn huge_start(
    process: &Process<'_>,
    request: &HugeRequest,
) -> Result<(usize, Option<Reservation>), Errno> {
    let bounds = process
        .address_space
        .native
        .task()
        .address_range()
        .map_err(mapping_error)?;
    let (start, reservation) = if request.exact {
        (request.address, None)
    } else {
        let hint = (request.address != 0).then_some(request.address);
        let reservation = process
            .address_space
            .reserve_huge(request.length, hint)
            .map_err(|error| huge_error(error, request.exact, request.replace))?;
        let range = reservation.range().ok_or(EIO)?;
        (range.start, Some(reservation))
    };
    // Validate the whole range before any block is allocated or materialized, and
    // before per-chunk arithmetic can overflow. The guard releases the range on
    // every error return below.
    let end = start.checked_add(request.length).ok_or(ENOMEM)?;
    if !start.is_multiple_of(HUGE_PAGE_SIZE) || start < bounds.start || end > bounds.end {
        return Err(ENOMEM);
    }
    Ok((start, reservation))
}

pub(super) fn check_huge_file_mapping(
    flags: usize,
    permissions: Permissions,
    description: &OpenFileDescription<'_>,
) -> Result<(), Errno> {
    // REFERENCE: https://man7.org/linux/man-pages/man2/mmap.2.html
    let mode = description.flags() & 3;
    if mode == O_WRONLY {
        // A mapping is readable unless PROT_NONE is requested, and a writable shared
        // mapping needs write access.
        return Err(EACCES);
    }
    if flags & 3 == MAP_SHARED && mode == O_RDONLY && permissions.writable() {
        return Err(EACCES);
    }
    Ok(())
}

pub(super) fn huge_block_address(start: usize, index: usize) -> Result<usize, Errno> {
    index
        .checked_mul(HUGE_PAGE_SIZE)
        .and_then(|offset| start.checked_add(offset))
        .ok_or(ENOMEM)
}

pub(super) fn allocate_huge(
    process: &Process<'_>,
    rights: Rights,
) -> Result<Handle<FrameAllocation>, Errno> {
    process
        .address_space
        .native
        .task()
        .allocate_contiguous(HUGE_FRAMES, HUGE_PAGE_SIZE, rights)
        .map_err(mapping_error)
}

pub(super) fn map_error(error: runtime::Error, exact: bool) -> Errno {
    match error {
        runtime::Error::Kernel(runtime::KernelError::Busy) if exact => EEXIST,
        error => mapping_error(error),
    }
}

pub(super) fn huge_error(error: runtime::Error, exact: bool, replace: bool) -> Errno {
    if replace {
        mapping_error(error)
    } else {
        map_error(error, exact)
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
        space
            .protect(range, permissions)
            .map_err(|error| match error {
                runtime::Error::Kernel(runtime::KernelError::InvalidArgument) => EINVAL,
                error => mapping_error(error),
            })?;
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
        PageBacking::File | PageBacking::ReadOnlyFile | PageBacking::BeyondEof(_)
    ) {
        return Err(ENOMEM);
    }
    let shared = matches!(first.page.backing, PageBacking::Shared);
    for mapped in &pages[1..] {
        if mapped.permissions != first.permissions
            || matches!(
                mapped.page.backing,
                PageBacking::File | PageBacking::ReadOnlyFile | PageBacking::BeyondEof(_)
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
        6 | 7 => Ok(Permissions::ReadWriteExecute),
        _ => Err(EINVAL),
    }
}

fn mapping_error(error: runtime::Error) -> Errno {
    match error {
        runtime::Error::Kernel(runtime::KernelError::Denied | runtime::KernelError::Busy) => EACCES,
        runtime::Error::Kernel(runtime::KernelError::Unsupported) => EOPNOTSUPP,
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
