// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Linux file accessibility checks.

use super::super::{
    Outcome,
    abi::{self, EACCES, EBADF, EINVAL, ENOENT, Errno, SyscallResult},
};
use super::{Pending, completion, filesystem_error, open, outcome};
use crate::{file::OpenFileDescription, process::Process};
use alloc::rc::Rc;
use runtime::{Call, Handle};

pub(super) const R_OK: u32 = 4;
pub(super) const W_OK: u32 = 2;
pub(super) const X_OK: u32 = 1;
pub(super) const F_OK: u32 = 0;

const AT_SYMLINK_NOFOLLOW: u32 = 0x100;
const AT_EACCESS: u32 = 0x200;
const AT_EMPTY_PATH: u32 = 0x1000;

/// An in-flight accessibility check against path or descriptor metadata.
pub(in crate::syscall) struct Access<'a> {
    source: Source,
    requested: u32,
    uid: u32,
    gid: u32,
    _anchor: Option<Rc<OpenFileDescription<'a>>>,
}

enum Source {
    Pending(fs::client::PendingMetadata),
    Ready(fs::Metadata),
}

pub(in crate::syscall) fn start<'a>(
    process: &Process<'a>,
    files: &fs::Client<'a>,
    number: usize,
    arguments: [usize; 6],
) -> Outcome<'a> {
    outcome(build(process, files, number, arguments))
}

fn build<'a>(
    process: &Process<'a>,
    files: &fs::Client<'a>,
    number: usize,
    arguments: [usize; 6],
) -> Result<Pending<'a>, Errno> {
    let requested = arguments[2] as u32;
    // REFERENCE: https://man7.org/linux/man-pages/man2/faccessat.2.html
    if requested & !(R_OK | W_OK | X_OK | F_OK) != 0 {
        return Err(EINVAL);
    }
    let flags = if number == abi::FACCESSAT2 {
        arguments[3] as u32
    } else {
        0
    };
    if flags & !(AT_SYMLINK_NOFOLLOW | AT_EACCESS | AT_EMPTY_PATH) != 0 {
        return Err(EINVAL);
    }
    let (uid, gid) = if flags & AT_EACCESS != 0 {
        (process.credentials.euid, process.credentials.egid)
    } else {
        (process.credentials.uid, process.credentials.gid)
    };
    let path = open::path_bytes(process, arguments[1])?;
    if path.is_empty() {
        if flags & AT_EMPTY_PATH == 0 {
            return Err(ENOENT);
        }
        return descriptor(process, arguments[0], requested, uid, gid);
    }
    let lookup = super::path::Lookup::from_name(process, arguments[0], path)?;
    let request = files
        .start_metadata_at(
            lookup.directory(),
            &lookup.name,
            flags & AT_SYMLINK_NOFOLLOW == 0,
        )
        .map_err(filesystem_error)?;
    Ok(Pending::Access(Access {
        source: Source::Pending(request),
        requested,
        uid,
        gid,
        _anchor: lookup.anchor,
    }))
}

fn descriptor<'a>(
    process: &Process<'a>,
    raw: usize,
    requested: u32,
    uid: u32,
    gid: u32,
) -> Result<Pending<'a>, Errno> {
    let index = usize::try_from(raw as i32).map_err(|_| EBADF)?;
    let description = Rc::clone(&process.files.get(index).ok_or(EBADF)?.description);
    let source = match description.as_ref() {
        OpenFileDescription::Regular { file, .. } => {
            Source::Pending(file.start_metadata().map_err(filesystem_error)?)
        }
        OpenFileDescription::Directory { session, .. } => {
            Source::Pending(session.start_metadata().map_err(filesystem_error)?)
        }
        OpenFileDescription::Socket { .. } | OpenFileDescription::SocketPair { .. } => {
            Source::Ready(synthetic(0o140777, 1))
        }
        OpenFileDescription::Pipe { end, .. } => Source::Ready(synthetic(0o010600, end.inode())),
        OpenFileDescription::Terminal { .. } => Source::Ready(synthetic(0o020600, 0)),
        OpenFileDescription::Eventfd { .. }
        | OpenFileDescription::Timerfd { .. }
        | OpenFileDescription::Signalfd { .. } => Source::Ready(synthetic(0o100600, 0x10)),
        OpenFileDescription::Rtc { .. } => Source::Ready(synthetic(0o020600, 0x11)),
        OpenFileDescription::VirtualDirectory { inode, .. } => {
            Source::Ready(synthetic(0o041777, *inode))
        }
        OpenFileDescription::MessageQueue { queue, .. }
        | OpenFileDescription::MqueueFile { queue, .. } => {
            let mode = if matches!(
                description.as_ref(),
                OpenFileDescription::MessageQueue { .. }
            ) {
                queue.mode()
            } else {
                0o444
            };
            Source::Ready(fs::Metadata {
                mode: 0o100000 | mode,
                inode: queue.inode(),
                links: 1,
                uid: queue.uid(),
                gid: queue.gid(),
                ..fs::Metadata::default()
            })
        }
        OpenFileDescription::Virtual { .. } => Source::Ready(synthetic(0o020666, 0)),
        OpenFileDescription::VirtualFile { inode, .. } => {
            Source::Ready(synthetic(0o100444, *inode))
        }
    };
    Ok(Pending::Access(Access {
        source,
        requested,
        uid,
        gid,
        _anchor: Some(description),
    }))
}

/// Builds the synthetic metadata of a descriptor without a filesystem node.
fn synthetic(mode: u32, inode: u64) -> fs::Metadata {
    fs::Metadata {
        inode,
        mode,
        links: 1,
        ..fs::Metadata::default()
    }
}

impl Access<'_> {
    pub fn call(&self) -> Option<&Handle<Call>> {
        match &self.source {
            Source::Pending(request) => Some(request.call()),
            Source::Ready(_) => None,
        }
    }

    pub fn poll(&mut self) -> Option<SyscallResult> {
        let info = match &mut self.source {
            Source::Pending(request) => match completion(request.try_complete())? {
                Ok(info) => info,
                Err(error) => return Some(Err(filesystem_error(error))),
            },
            Source::Ready(info) => *info,
        };
        Some(permit(&info, self.requested, self.uid, self.gid))
    }
}

/// Checks the requested access bits against one metadata record.
pub(super) fn permit(info: &fs::Metadata, requested: u32, uid: u32, gid: u32) -> SyscallResult {
    if requested == F_OK {
        return Ok(0);
    }
    let permissions = info.mode & 0o777;
    let directory = info.mode & 0o170000 == 0o040000;
    if uid == 0 {
        // Root bypasses read and write checks; execute still needs a bit on files.
        if requested & X_OK != 0 && !directory && permissions & 0o111 == 0 {
            return Err(EACCES);
        }
        return Ok(0);
    }
    let (class, shift) = if uid == info.uid {
        (0o700, 6)
    } else if gid == info.gid {
        (0o070, 3)
    } else {
        (0o007, 0)
    };
    let granted = (permissions & class) >> shift;
    let required = requested & 0o7;
    if granted & required == required {
        Ok(0)
    } else {
        Err(EACCES)
    }
}
