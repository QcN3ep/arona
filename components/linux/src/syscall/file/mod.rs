// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Linux filesystem calls and asynchronous completion.

// TODO: Support file change notifications.

mod access;
pub(super) mod buffer;
mod directory;
mod eventfd;
mod io;
mod metadata;
mod mount;
mod mounts;
pub(crate) mod open;
pub(crate) mod path;
mod pipe;
mod poll;
mod resolve;
mod rtc;
mod sendfile;
mod signalfd;
mod statistics;
mod terminal;
mod timerfd;
mod times;
pub(crate) mod virtual_file;

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
pub(super) use open::{openat, path_bytes};

pub(super) const O_CLOEXEC: u32 = 0x80000;

pub(super) enum Pending<'a> {
    Completed(SyscallResult),
    Resolve(alloc::boxed::Box<resolve::Resolve<'a>>),
    TerminalChange(terminal::Change),
    Terminal(terminal::Read<'a>),
    TerminalOutput(terminal::Output),
    TerminalWrite(terminal::Write<'a>),
    Pipe(pipe::PipeIo<'a>),
    Poll(poll::Poll<'a>),
    Open(open::Open<'a>),
    Mount(mount::Mount<'a>),
    MountTable(mounts::Read),
    Path(path::PathOperation<'a>),
    Eventfd(eventfd::Pending<'a>),
    Timerfd(timerfd::Read<'a>),
    Signalfd(signalfd::Read<'a>),
    Read(io::Read<'a>),
    Write(io::Write<'a>),
    Seek(io::Seek<'a>),
    Access(access::Access<'a>),
    Sendfile(sendfile::Sendfile<'a>),
    Utimens(times::Utimens<'a>),
    GlobalSync {
        request: fs::client::Pending<()>,
    },
    Stat(metadata::Stat<'a>),
    Statistics(statistics::Statistics<'a>),
    Directory(alloc::boxed::Box<directory::ReadDirectory<'a>>),
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
    services: &crate::Services<'a>,
    number: usize,
    arguments: [usize; 6],
) -> Option<Outcome<'a>> {
    if let Some(outcome) = resolve::start(process, number, arguments) {
        return Some(outcome);
    }
    dispatch_resolved(
        process,
        index,
        files,
        services,
        number,
        arguments,
        &mut resolve::Prepared::empty(),
    )
}

fn dispatch_resolved<'a>(
    process: &mut Process<'a>,
    index: usize,
    files: &fs::Client<'a>,
    services: &crate::Services<'a>,
    number: usize,
    arguments: [usize; 6],
    paths: &mut resolve::Prepared<'a>,
) -> Option<Outcome<'a>> {
    if let Some(outcome) = path::dispatch(process, files, number, arguments, paths) {
        return Some(outcome);
    }
    Some(match number {
        abi::IOCTL if rtc::is_rtc(process, arguments[0]) => Outcome::Complete(rtc::ioctl(
            process,
            services,
            arguments[0],
            arguments[1],
            arguments[2],
        )),
        abi::IOCTL => terminal::start(process, index, arguments[0], arguments[1], arguments[2]),
        abi::PIPE2 => Outcome::Complete(pipe::create(process, arguments[0], arguments[1])),
        abi::PPOLL => poll::start(process, arguments, false),
        abi::PSELECT6 => poll::start(process, arguments, true),
        abi::DUP => Outcome::Complete(duplicate(process, arguments[0], 0, false)),
        abi::DUP3 => Outcome::Complete(dup3(process, arguments[0], arguments[1], arguments[2])),
        abi::FCNTL => Outcome::Complete(fcntl(process, arguments[0], arguments[1], arguments[2])),
        abi::OPENAT => openat(process, files, services, arguments[2], arguments[3], paths),
        abi::CLOSE => Outcome::Complete(close(process, arguments[0])),
        abi::FSTAT => fstat(process, arguments[0], arguments[1]),
        abi::STATFS | abi::FSTATFS => statistics::start(
            process,
            files,
            arguments[0],
            arguments[1],
            number == abi::FSTATFS,
            paths,
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
            paths,
        ),
        abi::STATX => metadata::statx(process, files, arguments, paths),
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
        abi::SYNC => outcome(
            files
                .start_sync()
                .map(|request| Pending::GlobalSync { request })
                .map_err(filesystem_error),
        ),
        abi::FACCESSAT | abi::FACCESSAT2 => access::start(process, files, number, arguments, paths),
        abi::UTIMENSAT => times::start(process, files, services, arguments, paths),
        abi::SENDFILE => sendfile::start(process, arguments),
        abi::UMASK => Outcome::Complete(Ok(core::mem::replace(
            &mut process.umask,
            arguments[0] as u16 & 0o777,
        ) as usize)),
        abi::MOUNT => mount::mount(process, files, services, arguments, paths),
        abi::UMOUNT2 => mount::umount2(process, files, services, arguments, paths),
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
    if descriptor.description.path_only() && !matches!(command as u32, 0..=3 | 1030) {
        return Err(EBADF);
    }
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

pub(crate) fn fd(raw: usize) -> Result<usize, Errno> {
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
            Self::Resolve(resolve) => resolve.interrupt(process),
            Self::Terminal(read) => Some(read.interrupt(process)),
            Self::TerminalOutput(output) => output.interrupt(),
            Self::TerminalWrite(_)
            | Self::Pipe(_)
            | Self::Eventfd(_)
            | Self::Timerfd(_)
            | Self::Signalfd(_) => Some((true, Err(abi::EINTR))),
            Self::Poll(poll) => Some((false, poll.interrupt(process))),
            _ => None,
        }
    }

    pub fn waits<'b>(&'b self, items: &mut alloc::vec::Vec<runtime::ipc::WaitItem<'b>>) {
        if let Some(call) = self.call() {
            items.push(runtime::ipc::WaitItem::new(call, runtime::ipc::READABLE));
        }
        match self {
            Self::Resolve(resolve) => resolve.waits(items),
            Self::Poll(poll) => poll.waits(items),
            Self::Terminal(read) => read.waits(items),
            Self::Timerfd(read) => read.waits(items),
            Self::Sendfile(sendfile) => sendfile.waits(items),
            _ => {}
        }
    }

    pub fn call(&self) -> Option<&Handle<Call>> {
        match self {
            Self::Completed(_) => None,
            Self::Resolve(resolve) => resolve.call(),
            Self::Terminal(_)
            | Self::TerminalWrite(_)
            | Self::Eventfd(_)
            | Self::Timerfd(_)
            | Self::Signalfd(_) => None,
            Self::TerminalOutput(output) => output.call(),
            Self::TerminalChange(change) => change.call(),
            Self::Pipe(_) | Self::Poll(_) => None,
            Self::Open(open) => Some(open.call()),
            Self::Mount(mount) => Some(mount.call()),
            Self::MountTable(read) => Some(read.call()),
            Self::Path(path) => Some(path.call()),
            Self::Read(read) => read.call(),
            Self::Write(write) => write.call(),
            Self::Seek(seek) => seek.call(),
            Self::Access(access) => access.call(),
            Self::Sendfile(sendfile) => sendfile.call(),
            Self::Utimens(utimens) => utimens.call(),
            Self::Stat(stat) => Some(stat.call()),
            Self::Statistics(statistics) => statistics.call(),
            Self::Directory(directory) => directory.call(),
            Self::Output { request, .. } => Some(request.call()),
            Self::Truncate { request, .. } => Some(request.call()),
            Self::Sync { request, .. } => Some(request.call()),
            Self::GlobalSync { request } => Some(request.call()),
        }
    }

    pub fn poll(
        &mut self,
        process: &mut Process<'a>,
        index: usize,
        files: &fs::Client<'a>,
        services: &crate::Services<'a>,
    ) -> Option<SyscallResult> {
        match self {
            Self::Completed(result) => Some(*result),
            Self::Resolve(resolve) => resolve.poll(process, index, files, services),
            Self::Terminal(read) => read.poll(process),
            Self::TerminalOutput(output) => output.poll(),
            Self::TerminalWrite(write) => write.poll(),
            Self::Eventfd(read) => read.poll(process),
            Self::Timerfd(read) => read.poll(process),
            Self::Signalfd(read) => read.poll(process, index),
            Self::TerminalChange(change) => change.poll(),
            Self::Pipe(pipe) => pipe.poll(process),
            Self::Poll(poll) => poll.poll(process, index),
            Self::Open(open) => open.poll(process, files),
            Self::Mount(mount) => mount.poll(),
            Self::MountTable(read) => read.poll(process, files),
            Self::Path(path) => path.poll(process),
            Self::Read(read) => read.poll(process),
            Self::Write(write) => write.poll(),
            Self::Seek(seek) => seek.poll(),
            Self::Access(access) => access.poll(),
            Self::Sendfile(sendfile) => sendfile.poll(process),
            Self::Utimens(utimens) => utimens.poll(files),
            Self::Stat(stat) => stat.poll(process),
            Self::Statistics(statistics) => statistics.poll(process),
            Self::Directory(directory) => directory.poll(process, files, services),
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
            Self::GlobalSync { request } => Some(
                completion(request.try_complete())?
                    .map(|()| 0)
                    .map_err(filesystem_error),
            ),
        }
    }
}

pub(crate) fn completion<T>(result: Result<T, fs::Error>) -> Option<Result<T, fs::Error>> {
    match result {
        Err(fs::Error::Runtime(runtime::Error::Kernel(runtime::KernelError::WouldBlock))) => None,
        result => Some(result),
    }
}

pub(crate) fn filesystem_error(error: fs::Error) -> Errno {
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
