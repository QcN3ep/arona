// SPDX-License-Identifier: MPL-2.0

//! Linux 文件状态查询与结构编码。

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
    address: usize,
    _description: Option<Rc<OpenFileDescription<'a>>>,
}

pub(in crate::syscall) fn fstat<'a>(
    process: &Process<'a>,
    raw: usize,
    address: usize,
) -> Outcome<'a> {
    let description = match fd(raw).ok().and_then(|fd| process.files.get(fd)) {
        Some(descriptor) => Rc::clone(&descriptor.description),
        None => return Outcome::Complete(Err(EBADF)),
    };
    let request = match description.as_ref() {
        OpenFileDescription::Regular { file, .. } => file.start_metadata(),
        OpenFileDescription::Directory { session, .. } => session.start_metadata(),
        OpenFileDescription::Output { .. } => {
            return Outcome::Complete(write_stat(
                process,
                address,
                fs::Metadata {
                    mode: 0o020600,
                    links: 1,
                    ..fs::Metadata::default()
                },
            ));
        }
        OpenFileDescription::Pipe { end, .. } => {
            return Outcome::Complete(write_stat(
                process,
                address,
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
                    address,
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
    if path == 0 && flags & 0x1000 != 0 {
        return empty_stat(process, files, directory, address);
    }
    let path = match open::path_bytes(process, path) {
        Ok(path) => path,
        Err(error) => return Outcome::Complete(Err(error)),
    };
    if path.is_empty() && flags & 0x1000 != 0 {
        return empty_stat(process, files, directory, address);
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
            address,
            _description: lookup.anchor,
        }))
    })();
    outcome(operation)
}

fn empty_stat<'a>(
    process: &Process<'a>,
    files: &fs::Client<'a>,
    directory: usize,
    address: usize,
) -> Outcome<'a> {
    if directory as i32 != -100 {
        return fstat(process, directory, address);
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
                    address,
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
        Some(write_stat(process, self.address, info))
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
