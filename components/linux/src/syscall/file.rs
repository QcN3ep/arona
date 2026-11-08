// SPDX-License-Identifier: MPL-2.0

use alloc::{rc::Rc, vec::Vec};
use fs::ReadAt;
use runtime::{Call, Handle};

use super::{
    Outcome,
    abi::{
        EACCES, EBADF, EFAULT, EINVAL, EIO, EISDIR, EMFILE, ENAMETOOLONG, ENOENT, ENOMEM, ENOTDIR,
        EOPNOTSUPP, EOVERFLOW, ESPIPE, Errno, SyscallResult,
    },
};
use crate::{
    elf::PAGE_SIZE,
    file::{Descriptor, FilePosition, OpenFileDescription},
    process::Process,
};

const AT_FDCWD: i32 = -100;
// REFERENCE: https://github.com/torvalds/linux/blob/master/include/uapi/asm-generic/fcntl.h
const O_DIRECTORY: u32 = 0x10000;
const O_CLOEXEC: u32 = 0x80000;
const PATH_MAX: usize = 4096;

pub(super) enum Pending<'a> {
    Output {
        request: devmgr::device::Pending<usize, usize>,
        _description: Rc<OpenFileDescription<'a>>,
    },
    Open {
        request: Open<'a>,
        path: Vec<u8>,
        flags: u32,
    },
    Read(Read<'a>),
    Seek(Seek<'a>),
}

pub(super) enum Open<'a> {
    File(fs::client::PendingOpen<'a>),
    Directory(fs::client::PendingDirectory<'a>),
}

pub(super) struct Read<'a> {
    description: Rc<OpenFileDescription<'a>>,
    address: usize,
    length: usize,
    offset: Option<usize>,
    position: Option<FilePosition<'a>>,
    request: Option<fs::client::PendingRead>,
}

pub(super) struct Seek<'a> {
    description: Rc<OpenFileDescription<'a>>,
    displacement: i64,
    whence: u32,
    position: Option<FilePosition<'a>>,
    request: Option<fs::client::PendingSize>,
}

pub(super) fn openat<'a>(
    process: &Process<'a>,
    files: &fs::Client<'a>,
    directory: usize,
    address: usize,
    flags: usize,
) -> Outcome<'a> {
    let request = (|| {
        let flags = flags as u32;
        if flags & 3 == 3 {
            return Err(EINVAL);
        }
        if flags & !(O_DIRECTORY | O_CLOEXEC | 0x8000) != 0 {
            // TODO: 接入可写打开、创建和截断。
            return Err(EOPNOTSUPP);
        }
        let path = path(process, address)?;
        if path[0] != b'/' && directory as i32 != AT_FDCWD {
            let descriptor = process.files.get(fd(directory)?).ok_or(EBADF)?;
            return Err(match descriptor.description.as_ref() {
                // TODO: 由原生目录会话提供相对路径查找。
                OpenFileDescription::Directory { .. } => EOPNOTSUPP,
                _ => ENOTDIR,
            });
        }
        let request = if flags & O_DIRECTORY != 0 || path.ends_with(b"/") {
            Open::Directory(
                files
                    .start_open_directory(&path)
                    .map_err(filesystem_error)?,
            )
        } else {
            Open::File(files.start_open(&path).map_err(filesystem_error)?)
        };
        Ok(Pending::Open {
            request,
            path,
            flags,
        })
    })();
    outcome(request)
}

pub(super) fn read<'a>(
    process: &Process<'a>,
    fd_raw: usize,
    address: usize,
    length: usize,
    offset: Option<usize>,
) -> Outcome<'a> {
    let operation = (|| {
        let descriptor = process.files.get(fd(fd_raw)?).ok_or(EBADF)?;
        match descriptor.description.as_ref() {
            OpenFileDescription::Output(_) => {
                return Err(if offset.is_some() { ESPIPE } else { EBADF });
            }
            OpenFileDescription::Directory { .. } => return Err(EISDIR),
            OpenFileDescription::Regular { .. } => {}
        }
        if length > isize::MAX as usize || offset.is_some_and(|offset| offset > i64::MAX as usize) {
            return Err(EINVAL);
        }
        Ok(Pending::Read(Read {
            description: Rc::clone(&descriptor.description),
            address,
            length: length.min(PAGE_SIZE - address % PAGE_SIZE),
            offset,
            position: None,
            request: None,
        }))
    })();
    outcome(operation)
}

pub(super) fn write<'a>(
    process: &Process<'a>,
    fd_raw: usize,
    address: usize,
    length: usize,
) -> Outcome<'a> {
    let operation = (|| {
        let descriptor = process.files.get(fd(fd_raw)?).ok_or(EBADF)?;
        let OpenFileDescription::Output(output) = descriptor.description.as_ref() else {
            return Err(EBADF);
        };
        if length > isize::MAX as usize {
            return Err(EINVAL);
        }
        if length == 0 {
            return Ok(None);
        }
        let mut buffer = [0; devmgr::uart::MAX_BYTES];
        let buffer = &mut buffer[..length.min(devmgr::uart::MAX_BYTES)];
        process
            .address_space
            .read(address, buffer)
            .map_err(|_| EFAULT)?;
        let request = output.start_write(buffer).map_err(|_| EIO)?;
        Ok(Some(Pending::Output {
            request,
            _description: Rc::clone(&descriptor.description),
        }))
    })();
    match operation {
        Ok(Some(pending)) => Outcome::Pending(pending),
        Ok(None) => Outcome::Complete(Ok(0)),
        Err(error) => Outcome::Complete(Err(error)),
    }
}

pub(super) fn seek<'a>(
    process: &Process<'a>,
    fd_raw: usize,
    offset: usize,
    whence: usize,
) -> Outcome<'a> {
    let operation = (|| {
        let descriptor = process.files.get(fd(fd_raw)?).ok_or(EBADF)?;
        match descriptor.description.as_ref() {
            OpenFileDescription::Output(_) => return Err(ESPIPE),
            OpenFileDescription::Directory { .. } => return Err(EOPNOTSUPP),
            OpenFileDescription::Regular { .. } => {}
        }
        if whence as u32 > 2 {
            return Err(EINVAL);
        }
        Ok(Pending::Seek(Seek {
            description: Rc::clone(&descriptor.description),
            displacement: offset as i64,
            whence: whence as u32,
            position: None,
            request: None,
        }))
    })();
    outcome(operation)
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
        // TODO: 随可写文件和 exec 接入其余 fcntl 操作。
        _ => Err(EOPNOTSUPP),
    }
}

fn fd(raw: usize) -> Result<usize, Errno> {
    usize::try_from(raw as i32).map_err(|_| EBADF)
}

fn path(process: &Process<'_>, mut address: usize) -> Result<Vec<u8>, Errno> {
    let mut path = Vec::new();
    while path.len() < PATH_MAX {
        let mut bytes = [0; 256];
        let length = bytes
            .len()
            .min(PAGE_SIZE - address % PAGE_SIZE)
            .min(PATH_MAX - path.len());
        process
            .address_space
            .read(address, &mut bytes[..length])
            .map_err(|_| EFAULT)?;
        let end = bytes[..length].iter().position(|byte| *byte == 0);
        path.extend_from_slice(&bytes[..end.unwrap_or(length)]);
        if end.is_some() {
            if path.is_empty() {
                return Err(ENOENT);
            }
            for part in path.split(|byte| *byte == b'/') {
                if part.len() > 255 {
                    return Err(ENAMETOOLONG);
                }
                // TODO: 原生路径解析支持符号链接与父目录遍历后接入 ..。
                if part == b".." {
                    return Err(EOPNOTSUPP);
                }
            }
            return Ok(path);
        }
        address = address.checked_add(length).ok_or(EFAULT)?;
    }
    Err(ENAMETOOLONG)
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
            Self::Output { request, .. } => Some(request.call()),
            Self::Open {
                request: Open::File(request),
                ..
            } => Some(request.call()),
            Self::Open {
                request: Open::Directory(request),
                ..
            } => Some(request.call()),
            Self::Read(read) => read.request.as_ref().map(fs::client::PendingRead::call),
            Self::Seek(seek) => seek.request.as_ref().map(fs::client::PendingSize::call),
        }
    }

    pub fn poll(
        &mut self,
        process: &mut Process<'a>,
        files: &fs::Client<'a>,
    ) -> Option<SyscallResult> {
        match self {
            Self::Output { request, .. } => match request.try_complete() {
                Err(devmgr::Error::Runtime(runtime::Error::Kernel(
                    runtime::KernelError::WouldBlock,
                ))) => None,
                result => Some(result.map_err(|_| EIO)),
            },
            Self::Open {
                request,
                path,
                flags,
            } => {
                let descriptor = match request {
                    Open::File(pending) => match completion(pending.try_complete())? {
                        Ok(file) => {
                            if file.len() > i64::MAX as usize {
                                return Some(Err(EOVERFLOW));
                            }
                            Descriptor::regular(file, *flags & !O_CLOEXEC, *flags & O_CLOEXEC != 0)
                        }
                        Err(fs::Error::IsDirectory) => {
                            return match files.start_open_directory(path) {
                                Ok(pending) => {
                                    *request = Open::Directory(pending);
                                    None
                                }
                                Err(error) => Some(Err(filesystem_error(error))),
                            };
                        }
                        Err(error) => return Some(Err(filesystem_error(error))),
                    },
                    Open::Directory(pending) => match completion(pending.try_complete())? {
                        Ok(directory) => Descriptor {
                            description: Rc::new(OpenFileDescription::Directory {
                                _session: directory,
                                flags: *flags & !O_CLOEXEC,
                            }),
                            close_on_exec: *flags & O_CLOEXEC != 0,
                        },
                        Err(error) => return Some(Err(filesystem_error(error))),
                    },
                };
                Some(process.files.insert(descriptor, 0).ok_or(EMFILE))
            }
            Self::Read(read) => read.poll(process),
            Self::Seek(seek) => seek.poll(),
        }
    }
}

impl Read<'_> {
    fn poll(&mut self, process: &Process<'_>) -> Option<SyscallResult> {
        if self.length == 0 {
            return Some(Ok(0));
        }
        let OpenFileDescription::Regular { file, .. } = self.description.as_ref() else {
            unreachable!()
        };
        if self.request.is_none() {
            let offset = if let Some(offset) = self.offset {
                offset
            } else {
                let position = self.description.position()?;
                let offset = position.get();
                self.position = Some(position);
                offset
            };
            match file.start_read_at(offset, self.length.min(i64::MAX as usize - offset)) {
                Ok(request) => self.request = Some(request),
                Err(error) => return Some(Err(filesystem_error(error))),
            }
        }
        let bytes = match completion(self.request.as_mut().unwrap().try_complete())? {
            Ok(bytes) => bytes,
            Err(error) => return Some(Err(filesystem_error(error))),
        };
        if process.address_space.write(self.address, &bytes).is_err() {
            return Some(Err(EFAULT));
        }
        if let Some(position) = &self.position {
            position.set(position.get() + bytes.len());
        }
        Some(Ok(bytes.len()))
    }
}

impl Seek<'_> {
    fn poll(&mut self) -> Option<SyscallResult> {
        if self.position.is_none() {
            self.position = Some(self.description.position()?);
        }
        let position = self.position.as_ref().unwrap();
        let base = match self.whence {
            0 => 0,
            1 => position.get(),
            _ => {
                let OpenFileDescription::Regular { file, .. } = self.description.as_ref() else {
                    unreachable!()
                };
                if self.request.is_none() {
                    match file.start_size() {
                        Ok(request) => self.request = Some(request),
                        Err(error) => return Some(Err(filesystem_error(error))),
                    }
                }
                match completion(self.request.as_mut().unwrap().try_complete())? {
                    Ok(size) => size,
                    Err(error) => return Some(Err(filesystem_error(error))),
                }
            }
        };
        let offset = base as i128 + i128::from(self.displacement);
        if offset < 0 {
            return Some(Err(EINVAL));
        }
        if offset > i128::from(i64::MAX) {
            return Some(Err(EOVERFLOW));
        }
        position.set(offset as usize);
        Some(Ok(offset as usize))
    }
}

fn completion<T>(result: Result<T, fs::Error>) -> Option<Result<T, fs::Error>> {
    match result {
        Err(fs::Error::Runtime(runtime::Error::Kernel(runtime::KernelError::WouldBlock))) => None,
        result => Some(result),
    }
}

fn filesystem_error(error: fs::Error) -> Errno {
    match error {
        fs::Error::NotFound => ENOENT,
        fs::Error::NotDirectory => ENOTDIR,
        fs::Error::IsDirectory => EISDIR,
        fs::Error::ReadOnly => EACCES,
        fs::Error::InvalidRequest => EINVAL,
        fs::Error::Unsupported => EOPNOTSUPP,
        fs::Error::Runtime(runtime::Error::Kernel(runtime::KernelError::NoMemory)) => ENOMEM,
        _ => EIO,
    }
}
