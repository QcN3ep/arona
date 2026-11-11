// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Linux filesystem calls and asynchronous completion.

// TODO: Support file change notifications.

mod buffer;
mod directory;
mod eventfd;
mod io;
mod metadata;
mod open;
pub(in crate::syscall) mod path;
mod pipe;
mod poll;
mod signalfd;
mod statistics;
mod terminal;
mod timerfd;
mod virtual_file;

use super::{
    Outcome, abi,
    abi::{EBADF, EINVAL, EIO, EMFILE, EOPNOTSUPP, Errno, SyscallResult},
};
use crate::{file::OpenFileDescription, process::Process};
use alloc::rc::Rc;
use runtime::{Call, Handle};

pub(super) use buffer::Buffer;
pub(super) use directory::read as getdents;
pub(super) use io::{read, seek, sync, truncate, write};
pub(super) use metadata::{fstat, statat};
pub(super) use open::openat;

pub(super) const O_CLOEXEC: u32 = 0x80000;

pub(super) enum Pending<'a> {
    TerminalChange(terminal::Change),
    Terminal(terminal::Read<'a>),
    Pipe(pipe::PipeIo<'a>),
    Poll(poll::Poll<'a>),
    Open(open::Open<'a>),
    Path(path::PathOperation<'a>),
    Eventfd(eventfd::Pending<'a>),
    Timerfd(timerfd::Read<'a>),
    Signalfd(signalfd::Read<'a>),
    Read(io::Read<'a>),
    Write(io::Write<'a>),
    Seek(io::Seek<'a>),
    Stat(metadata::Stat<'a>),
    Statistics(statistics::Statistics<'a>),
    Directory(directory::ReadDirectory<'a>),
    Output {
        request: devmgr::device::Pending<usize, usize>,
        consumed: usize,
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
    index: usize,
    files: &fs::Client<'a>,
    services: &crate::Services<'_>,
    number: usize,
    arguments: [usize; 6],
) -> Option<Outcome<'a>> {
    if let Some(outcome) = path::dispatch(process, files, number, arguments) {
        return Some(outcome);
    }
    Some(match number {
        abi::IOCTL => terminal::start(process, index, arguments[0], arguments[1], arguments[2]),
        abi::PIPE2 => Outcome::Complete(pipe::create(process, arguments[0], arguments[1])),
        abi::PPOLL => poll::start(process, arguments, false),
        abi::PSELECT6 => poll::start(process, arguments, true),
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
        abi::STATFS | abi::FSTATFS => statistics::start(
            process,
            files,
            arguments[0],
            arguments[1],
            number == abi::FSTATFS,
        ),
        abi::SIGNALFD4 => Outcome::Complete(signalfd::create(
            process,
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        )),
        abi::TIMERFD_CREATE => {
            Outcome::Complete(timerfd::create(process, arguments[0], arguments[1]))
        }
        abi::TIMERFD_SETTIME => Outcome::Complete(timerfd::settime(
            process,
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        )),
        abi::TIMERFD_GETTIME => {
            Outcome::Complete(timerfd::gettime(process, arguments[0], arguments[1]))
        }
        abi::EVENTFD2 => {
            Outcome::Complete(eventfd::create(process, arguments[0] as u32, arguments[1]))
        }
        abi::GETDENTS64 => getdents(process, arguments[0], arguments[1], arguments[2]),
        abi::NEWFSTATAT => statat(
            process,
            files,
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        ),
        abi::STATX => metadata::statx(process, files, arguments),
        abi::LSEEK => seek(process, arguments[0], arguments[1], arguments[2]),
        abi::READ | abi::PREAD64 | abi::READV | abi::PREADV => read(
            process,
            index,
            arguments[0],
            io_buffer(number, &arguments),
            matches!(number, abi::PREAD64 | abi::PREADV).then_some(arguments[3]),
            services,
        ),
        abi::WRITE | abi::PWRITE64 | abi::WRITEV | abi::PWRITEV => write(
            process,
            index,
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
            if flags & (0x2000 | 0x4000 | 0x40000) != 0 {
                return Err(EOPNOTSUPP);
            }
            descriptor.description.set_flags(flags);
            Ok(0)
        }
        1031 => pipe::set_capacity(&descriptor.description, argument as u32 as usize),
        1032 => pipe::capacity(&descriptor.description),
        // TODO: Integrate file locks and the remaining fcntl operations.
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
    pub fn interrupt(&self, process: &Process<'_>) -> Option<(bool, SyscallResult)> {
        match self {
            Self::Terminal(read) => Some(read.interrupt(process)),
            Self::Pipe(_) | Self::Eventfd(_) | Self::Timerfd(_) | Self::Signalfd(_) => {
                Some((true, Err(abi::EINTR)))
            }
            Self::Poll(poll) => Some((false, poll.interrupt(process))),
            _ => None,
        }
    }

    pub fn waits<'b>(&'b self, items: &mut alloc::vec::Vec<runtime::ipc::WaitItem<'b>>) {
        if let Some(call) = self.call() {
            items.push(runtime::ipc::WaitItem::new(call, runtime::ipc::READABLE));
        }
        match self {
            Self::Poll(poll) => poll.waits(items),
            Self::Terminal(read) => read.waits(items),
            Self::Timerfd(read) => read.waits(items),
            _ => {}
        }
    }

    pub fn call(&self) -> Option<&Handle<Call>> {
        match self {
            Self::Terminal(_) | Self::Eventfd(_) | Self::Timerfd(_) | Self::Signalfd(_) => None,
            Self::TerminalChange(change) => Some(change.call()),
            Self::Pipe(_) | Self::Poll(_) => None,
            Self::Open(open) => Some(open.call()),
            Self::Path(path) => Some(path.call()),
            Self::Read(read) => read.call(),
            Self::Write(write) => write.call(),
            Self::Seek(seek) => seek.call(),
            Self::Stat(stat) => Some(stat.call()),
            Self::Statistics(statistics) => Some(statistics.call()),
            Self::Directory(directory) => directory.call(),
            Self::Output { request, .. } => Some(request.call()),
            Self::Truncate { request, .. } => Some(request.call()),
            Self::Sync { request, .. } => Some(request.call()),
        }
    }

    pub fn poll(
        &mut self,
        process: &mut Process<'a>,
        index: usize,
        files: &fs::Client<'a>,
    ) -> Option<SyscallResult> {
        match self {
            Self::Terminal(read) => read.poll(process),
            Self::Eventfd(read) => read.poll(process),
            Self::Timerfd(read) => read.poll(process),
            Self::Signalfd(read) => read.poll(process, index),
            Self::TerminalChange(change) => change.poll(),
            Self::Pipe(pipe) => pipe.poll(process),
            Self::Poll(poll) => poll.poll(process, index),
            Self::Open(open) => open.poll(process, files),
            Self::Path(path) => path.poll(process),
            Self::Read(read) => read.poll(process),
            Self::Write(write) => write.poll(),
            Self::Seek(seek) => seek.poll(),
            Self::Stat(stat) => stat.poll(process),
            Self::Statistics(statistics) => statistics.poll(process),
            Self::Directory(directory) => directory.poll(process),
            Self::Output {
                request, consumed, ..
            } => match request.try_complete() {
                Err(devmgr::Error::Runtime(runtime::Error::Kernel(
                    runtime::KernelError::WouldBlock,
                ))) => None,
                result => Some(result.map(|_| *consumed).map_err(|_| EIO)),
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
