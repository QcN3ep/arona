// SPDX-License-Identifier: MPL-2.0

//! Linux 路径与文件打开选项。

use alloc::{rc::Rc, vec::Vec};
use core::cell::Cell;
use fs::ReadAt;
use runtime::{Call, Handle};

use super::super::{
    Outcome,
    abi::{
        EBADF, EFAULT, EINVAL, EISDIR, EMFILE, ENAMETOOLONG, ENOENT, ENOTDIR, EOPNOTSUPP,
        EOVERFLOW, Errno, SyscallResult,
    },
};
use super::{O_CLOEXEC, Pending, completion, fd, filesystem_error, outcome};
use crate::{
    elf::PAGE_SIZE,
    file::{Descriptor, O_APPEND, O_DSYNC, O_NONBLOCK, OpenFileDescription},
    process::Process,
};

const AT_FDCWD: i32 = -100;
// REFERENCE: https://github.com/torvalds/linux/blob/master/include/uapi/asm-generic/fcntl.h
const O_CREAT: u32 = 0x40;
const O_EXCL: u32 = 0x80;
const O_TRUNC: u32 = 0x200;
const O_DIRECTORY: u32 = 0x10000;
const PATH_MAX: usize = 4096;

pub(in crate::syscall) struct Open<'a> {
    request: Request<'a>,
    path: Vec<u8>,
    flags: u32,
}

enum Request<'a> {
    File(fs::client::PendingOpen<'a>),
    Directory(fs::client::PendingDirectory<'a>),
}

pub(in crate::syscall) fn openat<'a>(
    process: &Process<'a>,
    files: &fs::Client<'a>,
    directory: usize,
    address: usize,
    flags: usize,
    mode: usize,
) -> Outcome<'a> {
    let operation = (|| {
        let mut flags = flags as u32;
        if flags & 0x100000 != 0 {
            flags |= O_DSYNC;
        }
        if flags & 3 == 3 || flags & (O_CREAT | O_DIRECTORY) == O_CREAT | O_DIRECTORY {
            return Err(EINVAL);
        }
        let supported = 3
            | O_CREAT
            | O_EXCL
            | O_TRUNC
            | O_APPEND
            | O_NONBLOCK
            | O_DSYNC
            | 0x100000
            | 0x8000
            | O_DIRECTORY
            | O_CLOEXEC;
        if flags & !supported != 0 {
            // TODO: 接入符号链接查找控制及其余打开选项。
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
            Request::Directory(
                files
                    .start_open_directory(&path)
                    .map_err(filesystem_error)?,
            )
        } else {
            Request::File(
                files
                    .start_open_with(
                        &path,
                        fs::OpenOptions {
                            write: flags & 3 != 0,
                            create: flags & O_CREAT != 0,
                            exclusive: flags & (O_CREAT | O_EXCL) == O_CREAT | O_EXCL,
                            truncate: flags & O_TRUNC != 0,
                            permissions: mode as u16 & 0o7777 & !process.umask,
                        },
                    )
                    .map_err(filesystem_error)?,
            )
        };
        Ok(Pending::Open(Open {
            request,
            path,
            flags,
        }))
    })();
    outcome(operation)
}

impl<'a> Open<'a> {
    pub fn call(&self) -> &Handle<Call> {
        match &self.request {
            Request::File(request) => request.call(),
            Request::Directory(request) => request.call(),
        }
    }

    pub fn poll(
        &mut self,
        process: &mut Process<'a>,
        files: &fs::Client<'a>,
    ) -> Option<SyscallResult> {
        let flags = self.flags & !(O_CREAT | O_EXCL | O_TRUNC | O_CLOEXEC);
        let close_on_exec = self.flags & O_CLOEXEC != 0;
        let descriptor = match &mut self.request {
            Request::File(pending) => match completion(pending.try_complete())? {
                Ok(file) => {
                    if file.len() > i64::MAX as usize {
                        return Some(Err(EOVERFLOW));
                    }
                    Descriptor::regular(file, flags, close_on_exec)
                }
                Err(fs::Error::IsDirectory) => {
                    if self.flags & (3 | O_CREAT | O_TRUNC) != 0 {
                        return Some(Err(EISDIR));
                    }
                    return match files.start_open_directory(&self.path) {
                        Ok(pending) => {
                            self.request = Request::Directory(pending);
                            None
                        }
                        Err(error) => Some(Err(filesystem_error(error))),
                    };
                }
                Err(error) => return Some(Err(filesystem_error(error))),
            },
            Request::Directory(pending) => match completion(pending.try_complete())? {
                Ok(directory) => {
                    if self.flags & (3 | O_CREAT | O_TRUNC) != 0 {
                        return Some(Err(EISDIR));
                    }
                    Descriptor {
                        description: Rc::new(OpenFileDescription::Directory {
                            session: directory,
                            flags: Cell::new(flags),
                            offset: Cell::new(0),
                            busy: Cell::new(false),
                        }),
                        close_on_exec,
                    }
                }
                Err(error) => return Some(Err(filesystem_error(error))),
            },
        };
        Some(process.files.insert(descriptor, 0).ok_or(EMFILE))
    }
}

pub(super) fn path(process: &Process<'_>, address: usize) -> Result<Vec<u8>, Errno> {
    let path = path_bytes(process, address)?;
    if path.is_empty() {
        return Err(ENOENT);
    }
    Ok(path)
}

pub(super) fn path_bytes(process: &Process<'_>, mut address: usize) -> Result<Vec<u8>, Errno> {
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
