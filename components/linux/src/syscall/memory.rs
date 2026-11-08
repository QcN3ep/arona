// SPDX-License-Identifier: MPL-2.0

use core::ops::Range;
use runtime::memory::Permissions;

use super::abi::{EACCES, EEXIST, EINVAL, EIO, ENOMEM, EOPNOTSUPP, Errno, SyscallResult};
use crate::{elf::PAGE_SIZE, memory::AddressSpace};

// REFERENCE: https://man7.org/linux/man-pages/man2/mmap.2.html
const MAP_PRIVATE: usize = 2;
const MAP_FIXED: usize = 0x10;
const MAP_ANONYMOUS: usize = 0x20;
const MAP_FIXED_NOREPLACE: usize = 0x100000;

pub(super) fn mmap(space: &AddressSpace, arguments: [usize; 6]) -> SyscallResult {
    let [address, length, protection, flags, _fd, offset] = arguments;
    if length == 0 || !offset.is_multiple_of(PAGE_SIZE) || flags & 3 == 0 {
        return Err(EINVAL);
    }
    let permissions = permissions(protection)?;
    if flags & 3 != MAP_PRIVATE
        || flags & MAP_ANONYMOUS == 0
        || flags & !(MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED | MAP_FIXED_NOREPLACE) != 0
        || flags & MAP_FIXED != 0 && flags & MAP_FIXED_NOREPLACE == 0
    {
        // TODO: 支持文件后备、共享映射和 MAP_FIXED 的原子替换。
        return Err(EOPNOTSUPP);
    }
    let length = length
        .checked_next_multiple_of(PAGE_SIZE)
        .filter(|length| *length <= isize::MAX as usize)
        .ok_or(ENOMEM)?;
    let exact = flags & (MAP_FIXED | MAP_FIXED_NOREPLACE) != 0;
    if exact && !address.is_multiple_of(PAGE_SIZE) {
        return Err(EINVAL);
    }
    if exact && address == 0 {
        return Err(ENOMEM);
    }
    space
        .map_anonymous(
            (address != 0).then_some(address & !(PAGE_SIZE - 1)),
            length,
            permissions,
            exact,
        )
        .map_err(|error| match error {
            runtime::Error::Kernel(runtime::KernelError::Busy) if exact => EEXIST,
            error => mapping_error(error),
        })
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
        space
            .native
            .protect(range, permissions)
            .map_err(mapping_error)?;
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
