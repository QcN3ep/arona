// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Read-only virtual files mounted at `/proc` and `/sys`.

use super::{Buffer, fd};
use crate::{
    file::OpenFileDescription,
    process::Process,
    syscall::abi::{EBADF, EINVAL, EOVERFLOW, EROFS, SyscallResult},
};
use alloc::vec::Vec;

#[derive(Clone, Copy)]
pub(super) enum File {
    ProcSelfStatus,
    ProcSelfCmdline,
    ProcVersion,
    ProcMounts,
    CpuOnline,
    OsRelease,
}

impl File {
    pub fn inode(self) -> u64 {
        match self {
            Self::ProcSelfStatus => 0x7072_0001,
            Self::ProcSelfCmdline => 0x7072_0002,
            Self::ProcVersion => 0x7072_0003,
            Self::ProcMounts => 0x7072_0004,
            Self::CpuOnline => 0x7379_0001,
            Self::OsRelease => 0x7379_0002,
        }
    }

    pub fn content(self, process: &Process<'_>) -> Vec<u8> {
        match self {
            Self::ProcSelfStatus => status(process),
            Self::ProcSelfCmdline => cmdline(process),
            Self::ProcVersion => alloc::format!(
                "Linux version {} (arona) #1 SMP\n",
                env!("CARGO_PKG_VERSION")
            )
            .into_bytes(),
            Self::ProcMounts => {
                b"proc /proc proc ro,relatime 0 0\nsysfs /sys sysfs ro,relatime 0 0\n".to_vec()
            }
            Self::CpuOnline => b"0\n".to_vec(),
            Self::OsRelease => alloc::format!("{}\n", env!("CARGO_PKG_VERSION")).into_bytes(),
        }
    }
}

pub(super) fn resolve(path: &[u8]) -> Option<File> {
    Some(match path {
        b"/proc/self/status" => File::ProcSelfStatus,
        b"/proc/self/cmdline" => File::ProcSelfCmdline,
        b"/proc/version" => File::ProcVersion,
        b"/proc/mounts" => File::ProcMounts,
        b"/sys/devices/system/cpu/online" => File::CpuOnline,
        b"/sys/kernel/osrelease" => File::OsRelease,
        _ => return None,
    })
}

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

fn status(process: &Process<'_>) -> Vec<u8> {
    let name = process
        .program
        .rsplit(|byte| *byte == b'/')
        .next()
        .unwrap_or(&process.program);
    alloc::format!(
        "Name:\t{}\nState:\tR (running)\nTgid:\t{pid}\nPid:\t{pid}\nPPid:\t{}\nUid:\t0\t0\t0\t0\nGid:\t0\t0\t0\t0\nThreads:\t{}\n",
        alloc::string::String::from_utf8_lossy(name),
        process.namespace.parent(process.pid),
        process.threads.len(),
        pid = process.pid,
    )
    .into_bytes()
}

fn cmdline(process: &Process<'_>) -> Vec<u8> {
    let mut data = Vec::new();
    for string in core::iter::once(&process.program).chain(process.argv.iter()) {
        data.extend_from_slice(string);
        data.push(0);
    }
    data
}
