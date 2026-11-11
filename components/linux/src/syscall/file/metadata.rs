// SPDX-License-Identifier: MPL-2.0

//! Linux file status queries and structure encoding.

use alloc::rc::Rc;
use runtime::{Call, Handle};

use super::super::{
    Outcome,
    abi::{EBADF, EFAULT, EINVAL, ENOENT, EOVERFLOW, Errno, SyscallResult},
};
use super::{Pending, completion, fd, filesystem_error, open, outcome};
use crate::{file::OpenFileDescription, process::Process};

pub(in crate::syscall) struct Stat<'a> {
    request: fs::client::PendingMetadata,
    buffer: StatBuffer,
    _description: Option<Rc<OpenFileDescription<'a>>>,
}

#[derive(Clone, Copy)]
enum StatBuffer {
    Stat(usize),
    Statx(usize),
}

pub(in crate::syscall) fn fstat<'a>(
    process: &Process<'a>,
    raw: usize,
    address: usize,
) -> Outcome<'a> {
    descriptor_stat(process, raw, StatBuffer::Stat(address))
}

fn descriptor_stat<'a>(process: &Process<'a>, raw: usize, buffer: StatBuffer) -> Outcome<'a> {
    let description = match fd(raw).ok().and_then(|fd| process.files.get(fd)) {
        Some(descriptor) => Rc::clone(&descriptor.description),
        None => return Outcome::Complete(Err(EBADF)),
    };
    let request = match description.as_ref() {
        OpenFileDescription::Regular { file, .. } => file.start_metadata(),
        OpenFileDescription::Directory { session, .. } => session.start_metadata(),
        OpenFileDescription::Terminal { .. } => {
            return Outcome::Complete(buffer.write(
                process,
                fs::Metadata {
                    mode: 0o020600,
                    links: 1,
                    ..fs::Metadata::default()
                },
            ));
        }
        OpenFileDescription::Pipe { end, .. } => {
            return Outcome::Complete(buffer.write(
                process,
                fs::Metadata {
                    device: 3,
                    inode: end.inode(),
                    mode: 0o010600,
                    links: 1,
                    ..fs::Metadata::default()
                },
            ));
        }
    };
    outcome(
        request
            .map(|request| {
                Pending::Stat(Stat {
                    request,
                    buffer,
                    _description: Some(description),
                })
            })
            .map_err(filesystem_error),
    )
}

pub(in crate::syscall) fn statat<'a>(
    process: &Process<'a>,
    files: &fs::Client<'a>,
    directory: usize,
    path: usize,
    address: usize,
    flags: usize,
) -> Outcome<'a> {
    if flags as u32 & !(0x100 | 0x800 | 0x1000) != 0 {
        return Outcome::Complete(Err(EINVAL));
    }
    lookup(
        process,
        files,
        directory,
        path,
        flags,
        StatBuffer::Stat(address),
    )
}

pub(in crate::syscall) fn statx<'a>(
    process: &Process<'a>,
    files: &fs::Client<'a>,
    arguments: [usize; 6],
) -> Outcome<'a> {
    let flags = arguments[2] as u32;
    let mask = arguments[3] as u32;
    if flags & !(0x100 | 0x800 | 0x1000 | 0x6000) != 0
        || flags & 0x6000 == 0x6000
        || mask & 0x8000_0000 != 0
    {
        return Outcome::Complete(Err(EINVAL));
    }
    lookup(
        process,
        files,
        arguments[0],
        arguments[1],
        flags as usize,
        StatBuffer::Statx(arguments[4]),
    )
}

fn lookup<'a>(
    process: &Process<'a>,
    files: &fs::Client<'a>,
    directory: usize,
    path: usize,
    flags: usize,
    buffer: StatBuffer,
) -> Outcome<'a> {
    if path == 0 && flags & 0x1000 != 0 {
        return empty_stat(process, files, directory, buffer);
    }
    let path = match open::path_bytes(process, path) {
        Ok(path) => path,
        Err(error) => return Outcome::Complete(Err(error)),
    };
    if path.is_empty() && flags & 0x1000 != 0 {
        return empty_stat(process, files, directory, buffer);
    }
    let operation = (|| {
        if path.is_empty() {
            return Err(ENOENT);
        }
        let lookup = super::path::Lookup::from_name(process, directory, path)?;
        Ok(Pending::Stat(Stat {
            request: files
                .start_metadata_at(lookup.directory(), &lookup.name, flags & 0x100 == 0)
                .map_err(filesystem_error)?,
            buffer,
            _description: lookup.anchor,
        }))
    })();
    outcome(operation)
}

fn empty_stat<'a>(
    process: &Process<'a>,
    files: &fs::Client<'a>,
    directory: usize,
    buffer: StatBuffer,
) -> Outcome<'a> {
    if directory as i32 != -100 {
        return descriptor_stat(process, directory, buffer);
    }
    let lookup = super::path::Lookup {
        name: b".".to_vec(),
        anchor: process.cwd.clone(),
    };
    outcome(
        files
            .start_metadata_at(lookup.directory(), b".", true)
            .map(|request| {
                Pending::Stat(Stat {
                    request,
                    buffer,
                    _description: lookup.anchor,
                })
            })
            .map_err(filesystem_error),
    )
}

impl Stat<'_> {
    pub fn call(&self) -> &Handle<Call> {
        self.request.call()
    }

    pub fn poll(&mut self, process: &Process<'_>) -> Option<SyscallResult> {
        let info = match completion(self.request.try_complete())? {
            Ok(info) => info,
            Err(error) => return Some(Err(filesystem_error(error))),
        };
        Some(self.buffer.write(process, info))
    }
}

impl StatBuffer {
    fn write(self, process: &Process<'_>, info: fs::Metadata) -> SyscallResult {
        match self {
            Self::Stat(address) => write_stat(process, address, info),
            Self::Statx(address) => write_statx(process, address, info),
        }
    }
}

// REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/include/uapi/asm-generic/stat.h?h=v6.12
fn write_stat(process: &Process<'_>, address: usize, info: fs::Metadata) -> Result<usize, Errno> {
    if info.size > i64::MAX as u64 || info.blocks > i64::MAX as u64 {
        return Err(EOVERFLOW);
    }
    let mut bytes = [0; 128];
    for (offset, value) in [
        (0, info.device),
        (8, info.inode),
        (48, info.size),
        (64, info.blocks),
        (72, info.accessed.seconds as u64),
        (80, u64::from(info.accessed.nanoseconds)),
        (88, info.modified.seconds as u64),
        (96, u64::from(info.modified.nanoseconds)),
        (104, info.changed.seconds as u64),
        (112, u64::from(info.changed.nanoseconds)),
    ] {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    for (offset, value) in [
        (16, info.mode),
        (20, info.links),
        (24, info.uid),
        (28, info.gid),
        (56, 4096),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    process
        .address_space
        .write(address, &bytes)
        .map_err(|_| EFAULT)?;
    Ok(0)
}

// REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/include/uapi/linux/stat.h?h=v6.12
fn write_statx(process: &Process<'_>, address: usize, info: fs::Metadata) -> SyscallResult {
    // TODO: Add birth time and mount ID.
    let mut bytes = [0; 256];
    let major = ((info.device >> 8) & 0xfff | (info.device >> 32) & 0xffff_f000) as u32;
    let minor = (info.device & 0xff | (info.device >> 12) & 0xffff_ff00) as u32;
    for (offset, value) in [(32, info.inode), (40, info.size), (48, info.blocks)] {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    for (offset, value) in [
        (0, 0x7ff),
        (4, 4096),
        (16, info.links),
        (20, info.uid),
        (24, info.gid),
        (136, major),
        (140, minor),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes[28..30].copy_from_slice(&(info.mode as u16).to_le_bytes());
    for (offset, time) in [
        (64, info.accessed),
        (96, info.changed),
        (112, info.modified),
    ] {
        bytes[offset..offset + 8].copy_from_slice(&time.seconds.to_le_bytes());
        bytes[offset + 8..offset + 12].copy_from_slice(&time.nanoseconds.to_le_bytes());
    }
    process
        .address_space
        .write(address, &bytes)
        .map_err(|_| EFAULT)?;
    Ok(0)
}
