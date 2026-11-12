// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Linux path and file open options.

use alloc::{rc::Rc, vec::Vec};
use fs::ReadAt;
use runtime::{Call, Handle};

use super::super::{
    Outcome,
    abi::{
        EBUSY, EFAULT, EINVAL, EIO, EISDIR, EMFILE, ENAMETOOLONG, ENOENT, ENOMEM, ENOTDIR,
        EOPNOTSUPP, EOVERFLOW, EROFS, Errno, SyscallResult,
    },
};
use super::{O_CLOEXEC, Pending, completion, filesystem_error, outcome, virtual_file};
use crate::{
    Services,
    elf::PAGE_SIZE,
    file::{
        Descriptor, O_APPEND, O_DSYNC, O_NONBLOCK, O_PATH, OpenFileDescription, Virtual,
        terminal::{Side, Terminal},
    },
    process::Process,
};

// REFERENCE: https://man7.org/linux/man-pages/man2/open.2.html
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
    path: Rc<Vec<u8>>,
    _directory: Option<Rc<OpenFileDescription<'a>>>,
}

pub(in crate::syscall) fn openat<'a>(
    process: &mut Process<'a>,
    files: &fs::Client<'a>,
    services: &Services<'_>,
    directory: usize,
    address: usize,
    flags: usize,
    mode: usize,
) -> Outcome<'a> {
    let operation = (|| {
        let mut flags = flags as u32;
        // An O_PATH request resolves the node without opening it for I/O, so every other
        // flag — including the access mode — is ignored rather than rejected.
        if flags & O_PATH != 0 {
            flags &= O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC;
        }
        if flags & 0x100000 != 0 {
            flags |= O_DSYNC;
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
            | O_PATH
            | O_CLOEXEC;
        if flags & !supported != 0 {
            // TODO: Support anonymous temporary files and signal-driven open options.
            return Err(EOPNOTSUPP);
        }
        if flags & 3 == 3 || flags & (O_CREAT | O_DIRECTORY) == O_CREAT | O_DIRECTORY {
            return Err(EINVAL);
        }
        let name = path(process, address)?;
        if matches!(
            name.as_slice(),
            b"/proc" | b"/proc/" | b"/proc/net" | b"/proc/net/" | b"/sys" | b"/sys/"
        ) {
            return Err(ENOTDIR);
        }
        if name.starts_with(b"/proc/") || name.starts_with(b"/sys/") {
            return open_snapshot(process, services, &name, flags)
                .map(|fd| Outcome::Complete(Ok(fd)));
        }
        if let Some(result) = super::super::mqueue::open_path(process, &name, flags) {
            return match result {
                Ok(fd) => Ok(Outcome::Complete(Ok(fd))),
                Err(error) => Ok(Outcome::Complete(Err(error))),
            };
        }
        if name == b"/dev/tty" {
            return open_terminal(process, flags, false).map(|fd| Outcome::Complete(Ok(fd)));
        }
        if name == b"/dev/console" {
            return open_terminal(process, flags, true).map(|fd| Outcome::Complete(Ok(fd)));
        }
        if name == b"/dev/ptmx" {
            return open_pty(process, None, flags).map(|fd| Outcome::Complete(Ok(fd)));
        }
        if let Some(number) = pty_slave_number(&name) {
            return open_pty(process, Some(number), flags).map(|fd| Outcome::Complete(Ok(fd)));
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
            b"/dev/rtc" | b"/dev/rtc0" | b"/dev/misc/rtc" => {
                return open_rtc(process, flags).map(|fd| Outcome::Complete(Ok(fd)));
            }
            _ => {}
        }
        let lookup = super::path::Lookup::from_name(process, directory, name)?;
        let path = Rc::new(lookup.name.clone());
        let request = files
            .start_open_at(
                lookup.directory(),
                &lookup.name,
                fs::OpenOptions {
                    path_only: flags & O_PATH != 0,
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
            path,
            _directory: lookup.anchor,
        }))))
    })();
    operation.unwrap_or_else(|error| Outcome::Complete(Err(error)))
}

fn open_snapshot(
    process: &mut Process<'_>,
    services: &Services<'_>,
    name: &[u8],
    flags: u32,
) -> Result<usize, Errno> {
    let Some(file) = virtual_file::resolve(process, name) else {
        return Err(ENOENT);
    };
    if flags & O_DIRECTORY != 0 {
        return Err(ENOTDIR);
    }
    if flags & (3 | O_CREAT | O_TRUNC) != 0 {
        return Err(EROFS);
    }
    let data = file.content(process, services)?;
    process
        .files
        .insert(
            Descriptor::snapshot(
                data,
                file.inode(),
                status_flags(flags, O_NONBLOCK | O_APPEND),
                flags & O_CLOEXEC != 0,
            ),
            0,
        )
        .ok_or(EMFILE)
}

fn open_virtual(process: &mut Process<'_>, flags: u32, kind: Virtual) -> Result<usize, Errno> {
    if flags & O_DIRECTORY != 0 {
        return Err(ENOTDIR);
    }
    process
        .files
        .insert(
            Descriptor::virtual_file(
                kind,
                status_flags(flags, 3 | O_NONBLOCK),
                flags & O_CLOEXEC != 0,
            ),
            0,
        )
        .ok_or(EMFILE)
}

fn open_rtc(process: &mut Process<'_>, flags: u32) -> Result<usize, Errno> {
    if flags & O_DIRECTORY != 0 {
        return Err(ENOTDIR);
    }
    process
        .files
        .insert(
            Descriptor::rtc(flags & (3 | O_NONBLOCK), flags & O_CLOEXEC != 0),
            0,
        )
        .ok_or(EMFILE)
}

fn open_terminal(process: &mut Process<'_>, flags: u32, console: bool) -> Result<usize, Errno> {
    if flags & O_DIRECTORY != 0 {
        return Err(ENOTDIR);
    }
    let terminal = if console {
        process
            .namespace
            .controlling_terminal(process.pid)
            .unwrap_or_else(|_| Rc::clone(&process.terminal))
    } else {
        process.namespace.controlling_terminal(process.pid)?
    };
    if terminal.exclusive() {
        return Err(EBUSY);
    }
    process
        .files
        .insert(
            Descriptor::terminal(
                terminal,
                status_flags(flags, 3 | O_NONBLOCK),
                flags & O_CLOEXEC != 0,
            ),
            0,
        )
        .ok_or(EMFILE)
}

fn open_pty(process: &mut Process<'_>, slave: Option<u32>, flags: u32) -> Result<usize, Errno> {
    if flags & O_DIRECTORY != 0 {
        return Err(ENOTDIR);
    }
    let terminal = match slave {
        Some(number) => {
            let pair = process.namespace.ptys.slave(number).ok_or(ENOENT)?;
            if !pair.unlocked() {
                return Err(EIO);
            }
            Rc::new(Terminal::pty(pair, Side::Slave))
        }
        None => {
            let pair = process.namespace.ptys.create().map_err(|()| ENOMEM)?;
            Rc::new(Terminal::pty(pair, Side::Master))
        }
    };
    process
        .files
        .insert(
            Descriptor::terminal(terminal, flags & (3 | O_NONBLOCK), flags & O_CLOEXEC != 0),
            0,
        )
        .ok_or(EMFILE)
}

/// Descriptor flags kept from an open: an `O_PATH` descriptor holds only its path identity,
/// dropping every I/O mode bit.
fn status_flags(flags: u32, mask: u32) -> u32 {
    if flags & O_PATH != 0 {
        O_PATH
    } else {
        flags & mask
    }
}

fn pty_slave_number(name: &[u8]) -> Option<u32> {
    let mut number = 0u32;
    let digits = name.strip_prefix(b"/dev/pts/")?;
    if digits.is_empty() {
        return None;
    }
    for &byte in digits {
        if !byte.is_ascii_digit() {
            return None;
        }
        number = number
            .checked_mul(10)?
            .checked_add(u32::from(byte - b'0'))?;
    }
    Some(number)
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
        let flags = status_flags(self.flags, !(O_CREAT | O_EXCL | O_TRUNC | O_CLOEXEC));
        let close_on_exec = self.flags & O_CLOEXEC != 0;
        let descriptor = match completion(self.request.try_complete())? {
            Ok(fs::client::OpenedNode::File(file)) => {
                if file.len() > i64::MAX as usize {
                    return Some(Err(EOVERFLOW));
                }
                Descriptor::regular(file, flags, close_on_exec, Some(Rc::clone(&self.path)))
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

pub(in crate::syscall) fn path_bytes(
    process: &Process<'_>,
    mut address: usize,
) -> Result<Vec<u8>, Errno> {
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
