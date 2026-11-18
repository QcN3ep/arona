// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Reads and offsets for snapshots of Linux virtual files.

use super::{Buffer, fd};
use crate::{
    file::OpenFileDescription,
    process::Process,
    syscall::abi::{EBADF, EINVAL, EOVERFLOW, EROFS, SyscallResult},
};

pub(super) fn is_virtual(process: &Process<'_>, raw: usize) -> bool {
    fd(raw)
        .ok()
        .and_then(|fd| process.files.get(fd))
        .is_some_and(|descriptor| {
            matches!(
                descriptor.description.as_ref(),
                OpenFileDescription::VirtualFile { .. }
            )
        })
}

pub(super) fn read(
    process: &Process<'_>,
    raw: usize,
    buffer: Buffer,
    offset: Option<usize>,
) -> SyscallResult {
    let descriptor = process.files.get(fd(raw)?).ok_or(EBADF)?;
    let OpenFileDescription::VirtualFile {
        data,
        offset: position,
        ..
    } = descriptor.description.as_ref()
    else {
        return Err(EBADF);
    };
    if descriptor.description.flags() & 3 == 1 || descriptor.description.path_only() {
        return Err(EBADF);
    }
    if offset.is_some_and(|offset| offset > i64::MAX as usize) {
        return Err(EINVAL);
    }
    let buffers = buffer.import(process)?;
    let start = offset.unwrap_or_else(|| position.get());
    let length = data.len().saturating_sub(start).min(buffers.length());
    if length == 0 {
        return Ok(0);
    }
    let mut bytes = data[start..start + length].to_vec();
    let count = buffers.scatter(process, &mut bytes)?;
    if offset.is_none() {
        position.set(start + count);
    }
    Ok(count)
}

pub(super) fn write(process: &Process<'_>, raw: usize) -> SyscallResult {
    process.files.get(fd(raw)?).ok_or(EBADF)?;
    Err(EROFS)
}

pub(super) fn seek(
    process: &Process<'_>,
    raw: usize,
    displacement: usize,
    whence: usize,
) -> SyscallResult {
    let descriptor = process.files.get(fd(raw)?).ok_or(EBADF)?;
    let OpenFileDescription::VirtualFile { data, offset, .. } = descriptor.description.as_ref()
    else {
        return Err(EBADF);
    };
    if descriptor.description.path_only() {
        return Err(EBADF);
    }
    let base = match whence {
        0 => 0,
        1 => offset.get(),
        2 => data.len(),
        _ => return Err(EINVAL),
    };
    let position = base as i128 + i128::from(displacement as i64);
    if position < 0 {
        return Err(EINVAL);
    }
    if position > i128::from(i64::MAX) {
        return Err(EOVERFLOW);
    }
    let position = position as usize;
    offset.set(position);
    Ok(position)
}
