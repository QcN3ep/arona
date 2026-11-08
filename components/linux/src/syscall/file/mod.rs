// SPDX-License-Identifier: MPL-2.0

//! Linux 文件系统调用与异步完成。

mod io;
mod open;

use super::{
    Outcome,
    abi::{EBADF, EINVAL, EIO, EMFILE, EOPNOTSUPP, Errno, SyscallResult},
};
use crate::{
    file::{O_NONBLOCK, OpenFileDescription},
    process::Process,
};
use alloc::rc::Rc;
use runtime::{Call, Handle};

pub(super) use io::{read, seek, sync, truncate, write};
pub(super) use open::openat;

pub(super) const O_CLOEXEC: u32 = 0x80000;

pub(super) enum Pending<'a> {
    Open(open::Open<'a>),
    Read(io::Read<'a>),
    Write(io::Write<'a>),
    Seek(io::Seek<'a>),
    Output {
        request: devmgr::device::Pending<usize, usize>,
        _description: Rc<OpenFileDescription<'a>>,
    },
    Truncate {
        request: fs::client::PendingTruncate,
        _description: Rc<OpenFileDescription<'a>>,
    },
    Sync {
        request: fs::client::Pending<()>,
        _description: Rc<OpenFileDescription<'a>>,
    },
}

pub(super) fn close(process: &mut Process<'_>, raw: usize) -> SyscallResult {
    process.files.close(fd(raw)?).then_some(0).ok_or(EBADF)
}

pub(super) fn duplicate(
    process: &mut Process<'_>,
    raw: usize,
    minimum: usize,
    close_on_exec: bool,
) -> SyscallResult {
    let descriptor = process
        .files
        .get(fd(raw)?)
        .ok_or(EBADF)?
        .duplicate(close_on_exec);
    process.files.insert(descriptor, minimum).ok_or(EMFILE)
}

pub(super) fn dup3(
    process: &mut Process<'_>,
    old: usize,
    new: usize,
    flags: usize,
) -> SyscallResult {
    let (old, new) = (fd(old)?, fd(new)?);
    if old == new || flags as u32 & !O_CLOEXEC != 0 {
        return Err(EINVAL);
    }
    let descriptor = process
        .files
        .get(old)
        .ok_or(EBADF)?
        .duplicate(flags as u32 & O_CLOEXEC != 0);
    process.files.replace(new, descriptor);
    Ok(new)
}

pub(super) fn fcntl(
    process: &mut Process<'_>,
    raw: usize,
    command: usize,
    argument: usize,
) -> SyscallResult {
    let index = fd(raw)?;
    let descriptor = process.files.get_mut(index).ok_or(EBADF)?;
    match command as u32 {
        0 | 1030 => {
            if argument > i32::MAX as usize {
                return Err(EINVAL);
            }
            duplicate(process, raw, argument, command as u32 == 1030)
        }
        1 => Ok(usize::from(descriptor.close_on_exec)),
        2 => {
            descriptor.close_on_exec = argument & 1 != 0;
            Ok(0)
        }
        3 => Ok(descriptor.description.flags() as usize),
        4 => {
            let flags = argument as u32;
            if flags & (0x2000 | 0x4000 | 0x40000) != 0
                || matches!(
                    descriptor.description.as_ref(),
                    OpenFileDescription::Output { .. }
                ) && flags & O_NONBLOCK != 0
            {
                return Err(EOPNOTSUPP);
            }
            descriptor.description.set_flags(flags);
            Ok(0)
        }
        // TODO: 接入文件锁与其余 fcntl 操作。
        _ => Err(EOPNOTSUPP),
    }
}

fn fd(raw: usize) -> Result<usize, Errno> {
    usize::try_from(raw as i32).map_err(|_| EBADF)
}

fn outcome<'a>(operation: Result<Pending<'a>, Errno>) -> Outcome<'a> {
    match operation {
        Ok(operation) => Outcome::Pending(operation),
        Err(error) => Outcome::Complete(Err(error)),
    }
}

impl<'a> Pending<'a> {
    pub fn call(&self) -> Option<&Handle<Call>> {
        match self {
            Self::Open(open) => Some(open.call()),
            Self::Read(read) => read.call(),
            Self::Write(write) => write.call(),
            Self::Seek(seek) => seek.call(),
            Self::Output { request, .. } => Some(request.call()),
            Self::Truncate { request, .. } => Some(request.call()),
            Self::Sync { request, .. } => Some(request.call()),
        }
    }

    pub fn poll(
        &mut self,
        process: &mut Process<'a>,
        files: &fs::Client<'a>,
    ) -> Option<SyscallResult> {
        match self {
            Self::Open(open) => open.poll(process, files),
            Self::Read(read) => read.poll(process),
            Self::Write(write) => write.poll(),
            Self::Seek(seek) => seek.poll(),
            Self::Output { request, .. } => match request.try_complete() {
                Err(devmgr::Error::Runtime(runtime::Error::Kernel(
                    runtime::KernelError::WouldBlock,
                ))) => None,
                result => Some(result.map_err(|_| EIO)),
            },
            Self::Truncate { request, .. } => Some(
                completion(request.try_complete())?
                    .map(|()| 0)
                    .map_err(filesystem_error),
            ),
            Self::Sync { request, .. } => Some(
                completion(request.try_complete())?
                    .map(|()| 0)
                    .map_err(filesystem_error),
            ),
        }
    }
}

fn completion<T>(result: Result<T, fs::Error>) -> Option<Result<T, fs::Error>> {
    match result {
        Err(fs::Error::Runtime(runtime::Error::Kernel(runtime::KernelError::WouldBlock))) => None,
        result => Some(result),
    }
}

fn filesystem_error(error: fs::Error) -> Errno {
    use super::abi::{EEXIST, EFBIG, EISDIR, ENOENT, ENOMEM, ENOSPC, ENOTDIR, EROFS};
    match error {
        fs::Error::NotFound => ENOENT,
        fs::Error::NotDirectory => ENOTDIR,
        fs::Error::IsDirectory => EISDIR,
        fs::Error::ReadOnly => EROFS,
        fs::Error::NoSpace => ENOSPC,
        fs::Error::AlreadyExists => EEXIST,
        fs::Error::FileTooLarge => EFBIG,
        fs::Error::InvalidRequest => EINVAL,
        fs::Error::Unsupported => EOPNOTSUPP,
        fs::Error::Runtime(runtime::Error::Kernel(runtime::KernelError::NoMemory)) => ENOMEM,
        _ => EIO,
    }
}
