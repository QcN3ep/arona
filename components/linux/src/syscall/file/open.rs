// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Linux path and file open options.

use alloc::{rc::Rc, vec::Vec};
use fs::ReadAt;
use runtime::{Call, Handle};

use super::super::{
    Outcome,
    abi::{
        EFAULT, EINVAL, EISDIR, EMFILE, ENAMETOOLONG, ENOENT, ENOTDIR, EOPNOTSUPP, EOVERFLOW,
        Errno, SyscallResult,
    },
};
use super::{O_CLOEXEC, Pending, completion, filesystem_error, outcome};
use crate::{
    elf::PAGE_SIZE,
    file::{Descriptor, O_APPEND, O_DSYNC, O_NONBLOCK, OpenFileDescription, Virtual},
    process::Process,
};

// REFERENCE: https://github.com/torvalds/linux/blob/master/include/uapi/asm-generic/fcntl.h
const O_CREAT: u32 = 0x40;
const O_EXCL: u32 = 0x80;
const O_TRUNC: u32 = 0x200;
const O_DIRECTORY: u32 = 0x10000;
const O_NOFOLLOW: u32 = 0x20000;
const PATH_MAX: usize = 4096;

pub(in crate::syscall) struct Open<'a> {
    request: fs::client::PendingNode<'a>,
    flags: u32,
    _directory: Option<Rc<OpenFileDescription<'a>>>,
}

pub(in crate::syscall) fn openat<'a>(
    process: &mut Process<'a>,
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
            | 0x100
            | O_TRUNC
            | O_APPEND
            | O_NONBLOCK
            | O_DSYNC
            | 0x100000
            | 0x8000
            | 0x40000
            | 0x4000
            | O_DIRECTORY
            | O_NOFOLLOW
            | O_CLOEXEC;
        if flags & !supported != 0 {
            // TODO: Integrate the remaining open options.
            return Err(EOPNOTSUPP);
        }
        let name = path(process, address)?;
        if name == b"/dev/tty" {
            return open_terminal(process, flags).map(|fd| Outcome::Complete(Ok(fd)));
        }
        match name.as_slice() {
            b"/dev/null" => {
                return open_virtual(process, flags, Virtual::Null)
                    .map(|fd| Outcome::Complete(Ok(fd)));
            }
            b"/dev/zero" => {
                return open_virtual(process, flags, Virtual::Zero)
                    .map(|fd| Outcome::Complete(Ok(fd)));
            }
            b"/dev/random" | b"/dev/urandom" => {
                return open_virtual(process, flags, Virtual::Random)
                    .map(|fd| Outcome::Complete(Ok(fd)));
            }
            _ => {}
        }
        let lookup = super::path::Lookup::from_name(process, directory, name)?;
        let request = files
            .start_open_at(
                lookup.directory(),
                &lookup.name,
                fs::OpenOptions {
                    write: flags & 3 != 0,
                    create: flags & O_CREAT != 0,
                    exclusive: flags & (O_CREAT | O_EXCL) == O_CREAT | O_EXCL,
                    truncate: flags & O_TRUNC != 0,
                    permissions: mode as u16 & 0o7777 & !process.umask,
                },
                flags & O_NOFOLLOW == 0,
                flags & O_DIRECTORY != 0,
            )
            .map_err(filesystem_error)?;
        Ok(outcome(Ok(Pending::Open(Open {
            request,
            flags,
            _directory: lookup.anchor,
        }))))
    })();
    operation.unwrap_or_else(|error| Outcome::Complete(Err(error)))
}

fn open_virtual(process: &mut Process<'_>, flags: u32, kind: Virtual) -> Result<usize, Errno> {
    if flags & O_DIRECTORY != 0 {
        return Err(ENOTDIR);
    }
    process
        .files
        .insert(
            Descriptor::virtual_file(kind, flags & (3 | O_NONBLOCK), flags & O_CLOEXEC != 0),
            0,
        )
        .ok_or(EMFILE)
}

fn open_terminal(process: &mut Process<'_>, flags: u32) -> Result<usize, Errno> {
    if flags & O_DIRECTORY != 0 {
        return Err(ENOTDIR);
    }
    let terminal = process.namespace.controlling_terminal(process.pid)?;
    process
        .files
        .insert(
            Descriptor::terminal(terminal, flags & (3 | O_NONBLOCK), flags & O_CLOEXEC != 0),
            0,
        )
        .ok_or(EMFILE)
}

impl<'a> Open<'a> {
    pub fn call(&self) -> &Handle<Call> {
        self.request.call()
    }

    pub fn poll(
        &mut self,
        process: &mut Process<'a>,
        _files: &fs::Client<'a>,
    ) -> Option<SyscallResult> {
        let flags = self.flags & !(O_CREAT | O_EXCL | O_TRUNC | O_CLOEXEC);
        let close_on_exec = self.flags & O_CLOEXEC != 0;
        let descriptor = match completion(self.request.try_complete())? {
            Ok(fs::client::OpenedNode::File(file)) => {
                if file.len() > i64::MAX as usize {
                    return Some(Err(EOVERFLOW));
                }
                Descriptor::regular(file, flags, close_on_exec)
            }
            Ok(fs::client::OpenedNode::Directory(session)) => {
                if self.flags & (3 | O_CREAT | O_TRUNC) != 0 {
                    return Some(Err(EISDIR));
                }
                super::path::directory_descriptor(session, flags, close_on_exec)
            }
            Err(error) => return Some(Err(filesystem_error(error))),
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
            }
            return Ok(path);
        }
        address = address.checked_add(length).ok_or(EFAULT)?;
    }
    Err(ENAMETOOLONG)
}
