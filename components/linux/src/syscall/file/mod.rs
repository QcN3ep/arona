// SPDX-License-Identifier: MPL-2.0

//! Linux 文件系统调用与异步完成。

mod buffer;
mod directory;
mod io;
mod metadata;
mod open;
mod path;

use super::{
    Outcome, abi,
    abi::{EBADF, EINVAL, EIO, EMFILE, EOPNOTSUPP, Errno, SyscallResult},
};
use crate::{
    file::{O_NONBLOCK, OpenFileDescription},
    process::Process,
};
use alloc::rc::Rc;
use runtime::{Call, Handle};

pub(super) use buffer::Buffer;
pub(super) use directory::read as getdents;
pub(super) use io::{read, seek, sync, truncate, write};
pub(super) use metadata::{fstat, statat};
pub(super) use open::openat;

pub(super) const O_CLOEXEC: u32 = 0x80000;

pub(super) enum Pending<'a> {
    Open(open::Open<'a>),
    Path(path::PathOperation<'a>),
    Read(io::Read<'a>),
    Write(io::Write<'a>),
    Seek(io::Seek<'a>),
    Stat(metadata::Stat<'a>),
    Directory(directory::ReadDirectory<'a>),
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

pub(super) fn dispatch<'a>(
    process: &mut Process<'a>,
    files: &fs::Client<'a>,
    number: usize,
    arguments: [usize; 6],
) -> Option<Outcome<'a>> {
    if let Some(outcome) = path::dispatch(process, files, number, arguments) {
        return Some(outcome);
    }
    Some(match number {
        abi::DUP => Outcome::Complete(duplicate(process, arguments[0], 0, false)),
        abi::DUP3 => Outcome::Complete(dup3(process, arguments[0], arguments[1], arguments[2])),
        abi::FCNTL => Outcome::Complete(fcntl(process, arguments[0], arguments[1], arguments[2])),
        abi::OPENAT => openat(
            process,
            files,
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        ),
        abi::CLOSE => Outcome::Complete(close(process, arguments[0])),
        abi::FSTAT => fstat(process, arguments[0], arguments[1]),
        abi::GETDENTS64 => getdents(process, arguments[0], arguments[1], arguments[2]),
        abi::NEWFSTATAT => statat(
            process,
            files,
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        ),
        abi::LSEEK => seek(process, arguments[0], arguments[1], arguments[2]),
        abi::READ | abi::PREAD64 | abi::READV | abi::PREADV => read(
            process,
            arguments[0],
            io_buffer(number, &arguments),
            matches!(number, abi::PREAD64 | abi::PREADV).then_some(arguments[3]),
        ),
        abi::WRITE | abi::PWRITE64 | abi::WRITEV | abi::PWRITEV => write(
            process,
            arguments[0],
            io_buffer(number, &arguments),
            matches!(number, abi::PWRITE64 | abi::PWRITEV).then_some(arguments[3]),
        ),
        abi::FTRUNCATE => truncate(process, arguments[0], arguments[1]),
        abi::FSYNC | abi::FDATASYNC => sync(process, arguments[0]),
        abi::UMASK => Outcome::Complete(Ok(core::mem::replace(
            &mut process.umask,
            arguments[0] as u16 & 0o777,
        ) as usize)),
        _ => return None,
    })
}

fn io_buffer(number: usize, arguments: &[usize; 6]) -> Buffer {
    if matches!(
        number,
        abi::READV | abi::PREADV | abi::WRITEV | abi::PWRITEV
    ) {
        Buffer::Vector {
            address: arguments[1],
            count: arguments[2],
        }
    } else {
        Buffer::Slice {
            address: arguments[1],
            length: arguments[2],
        }
    }
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
        Ok(operation) => Outcome::Pending(super::Operation::File(operation)),
        Err(error) => Outcome::Complete(Err(error)),
    }
}

impl<'a> Pending<'a> {
    pub fn call(&self) -> Option<&Handle<Call>> {
        match self {
            Self::Open(open) => Some(open.call()),
            Self::Path(path) => Some(path.call()),
            Self::Read(read) => read.call(),
            Self::Write(write) => write.call(),
            Self::Seek(seek) => seek.call(),
            Self::Stat(stat) => Some(stat.call()),
            Self::Directory(directory) => directory.call(),
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
            Self::Path(path) => path.poll(process),
            Self::Read(read) => read.poll(process),
            Self::Write(write) => write.poll(),
            Self::Seek(seek) => seek.poll(),
            Self::Stat(stat) => stat.poll(process),
            Self::Directory(directory) => directory.poll(process),
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

pub(super) fn completion<T>(result: Result<T, fs::Error>) -> Option<Result<T, fs::Error>> {
    match result {
        Err(fs::Error::Runtime(runtime::Error::Kernel(runtime::KernelError::WouldBlock))) => None,
        result => Some(result),
    }
}

pub(super) fn filesystem_error(error: fs::Error) -> Errno {
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
        fs::Error::TooManyLinks => super::abi::ELOOP,
        fs::Error::NameTooLong => super::abi::ENAMETOOLONG,
        fs::Error::Busy => super::abi::EBUSY,
        fs::Error::NotEmpty => super::abi::ENOTEMPTY,
        fs::Error::CrossDevice => super::abi::EXDEV,
        fs::Error::Unsupported => EOPNOTSUPP,
        fs::Error::Runtime(runtime::Error::Kernel(runtime::KernelError::NoMemory)) => ENOMEM,
        _ => EIO,
    }
}
