// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Linux `mount` and `umount2` calls backed by the file service namespace.

use super::super::abi::{EINVAL, ENODEV, ENOENT, ENOTDIR, EOPNOTSUPP, EPERM, Errno, SyscallResult};
use super::{Pending, completion, filesystem_error, open, outcome};
use crate::{file::OpenFileDescription, process::Process, syscall::Outcome};
use alloc::{rc::Rc, vec::Vec};
use runtime::{Call, Handle};

// REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/include/uapi/linux/mount.h?h=v6.12
const MS_SILENT: u32 = 1 << 15;
const MS_NOATIME: u32 = 1 << 10;
const MS_NODIRATIME: u32 = 1 << 11;
const MS_RELATIME: u32 = 1 << 21;
const MS_STRICTATIME: u32 = 1 << 24;
const MS_LAZYTIME: u32 = 1 << 25;
// MS_NOSUID, MS_NODEV, and MS_NOEXEC change how a mount is interpreted; the
// namespace does not store or enforce them yet, so reject them instead of
// accepting flags with no effect. SILENT and the atime flags have no distinct
// behavior for an in-memory volume.
const SUPPORTED_FLAGS: u32 =
    MS_SILENT | MS_NOATIME | MS_NODIRATIME | MS_RELATIME | MS_STRICTATIME | MS_LAZYTIME;

// A user mount is checked in two layers. The Linux layer restricts the canonical
// target to a writable root and its mount call is gated by the process mount
// capability. The file service independently requires the target to resolve
// below an existing mount, which keeps a client from shadowing the boot archive
// even if it bypasses this check; doing the first check here avoids a request the
// service would deny.
const USER_MOUNT_ROOTS: &[&[u8]] = &[b"/tmp", b"/var"];

#[derive(Clone, Copy)]
enum Request {
    Mount,
    Unmount,
}

#[derive(Clone, Copy)]
enum Operation {
    Mount(MountKind),
    Unmount,
}

#[derive(Clone, Copy)]
enum MountKind {
    Tmpfs,
    Hugepages,
}

impl MountKind {
    fn from_name(name: &[u8]) -> Option<Self> {
        match name {
            b"tmpfs" => Some(Self::Tmpfs),
            b"hugetlbfs" => Some(Self::Hugepages),
            _ => None,
        }
    }
}

pub(in crate::syscall) struct Mount<'a> {
    stage: Stage<'a>,
    operation: Operation,
    _directory: Option<fs::Directory<'a>>,
    _anchor: Option<Rc<OpenFileDescription<'a>>>,
    mounts: fs::MountControl<'a>,
}

enum Stage<'a> {
    Open(fs::client::PendingNode<'a>),
    Path(fs::client::PendingRead),
    Request(fs::client::Pending<()>),
}

pub(super) fn mount<'a>(
    process: &Process<'a>,
    files: &fs::Client<'a>,
    services: &crate::Services<'a>,
    arguments: [usize; 6],
    paths: &mut super::resolve::Prepared<'a>,
) -> Outcome<'a> {
    start(process, files, services, arguments, Request::Mount, paths)
}

pub(super) fn umount2<'a>(
    process: &Process<'a>,
    files: &fs::Client<'a>,
    services: &crate::Services<'a>,
    arguments: [usize; 6],
    paths: &mut super::resolve::Prepared<'a>,
) -> Outcome<'a> {
    start(process, files, services, arguments, Request::Unmount, paths)
}

fn start<'a>(
    process: &Process<'a>,
    files: &fs::Client<'a>,
    services: &crate::Services<'a>,
    arguments: [usize; 6],
    request: Request,
    paths: &mut super::resolve::Prepared<'a>,
) -> Outcome<'a> {
    let request = (|| {
        check_privileged(process)?;
        let (_target, operation) = match request {
            Request::Mount => {
                let _source = optional_string(process, arguments[0])?;
                let target = open::path_bytes(process, arguments[1])?;
                if target.is_empty() {
                    return Err(ENOENT);
                }
                let filesystem = optional_string(process, arguments[2])?;
                let kind = MountKind::from_name(&filesystem).ok_or(ENODEV)?;
                let flags = u32::try_from(arguments[3]).map_err(|_| EINVAL)?;
                if flags & !SUPPORTED_FLAGS != 0 {
                    return Err(EINVAL);
                }
                if !optional_string(process, arguments[4])?.is_empty() {
                    return Err(EINVAL);
                }
                (target, Operation::Mount(kind))
            }
            Request::Unmount => {
                let target = open::path_bytes(process, arguments[0])?;
                if target.is_empty() {
                    return Err(ENOENT);
                }
                // MNT_FORCE, MNT_DETACH, MNT_EXPIRE, and UMOUNT_NOFOLLOW have no
                // distinct behavior in a namespace without stacked mounts, so reject
                // them.
                if arguments[1] != 0 {
                    return Err(EINVAL);
                }
                (target, Operation::Unmount)
            }
        };
        let mounts = services
            .mounts
            .as_ref()
            .ok_or(EOPNOTSUPP)?
            .try_clone()
            .map_err(filesystem_error)?;
        // Resolve the target through the file service so relative paths, symbolic
        // links, and the mount's own directory name all reach the same canonical
        // path that mount table lookups use.
        let resolved = paths.take()?;
        if resolved.local {
            return Err(EPERM);
        }
        let lookup = resolved.lookup;
        let request = files
            .start_open_at(
                lookup.directory(),
                &lookup.name,
                fs::OpenOptions {
                    path_only: false,
                    write: false,
                    create: false,
                    exclusive: false,
                    truncate: false,
                    permissions: 0,
                },
                true,
                true,
            )
            .map_err(filesystem_error)?;
        Ok(Mount {
            stage: Stage::Open(request),
            operation,
            _directory: None,
            _anchor: lookup.anchor,
            mounts,
        })
    })();
    match request {
        Ok(request) => outcome(Ok(Pending::Mount(request))),
        Err(error) => Outcome::Complete(Err(error)),
    }
}

impl<'a> Mount<'a> {
    pub fn call(&self) -> &Handle<Call> {
        match &self.stage {
            Stage::Open(request) => request.call(),
            Stage::Path(request) => request.call(),
            Stage::Request(request) => request.call(),
        }
    }

    pub fn poll(&mut self) -> Option<SyscallResult> {
        match &mut self.stage {
            Stage::Open(request) => {
                let directory = match completion(request.try_complete())? {
                    Ok(fs::client::OpenedNode::Directory(directory)) => directory,
                    Ok(fs::client::OpenedNode::File(_)) => return Some(Err(ENOTDIR)),
                    Err(error) => return Some(Err(filesystem_error(error))),
                };
                let request = match directory.start_path() {
                    Ok(request) => request,
                    Err(error) => return Some(Err(filesystem_error(error))),
                };
                self.stage = Stage::Path(request);
                self._directory = Some(directory);
            }
            Stage::Path(request) => {
                let path = match completion(request.try_complete())? {
                    Ok(path) => path,
                    Err(error) => return Some(Err(filesystem_error(error))),
                };
                // Retaining the directory session would make its own unmount busy.
                self._directory = None;
                let request = match self.operation {
                    Operation::Mount(_) if !mount_target_allowed(&path) => return Some(Err(EPERM)),
                    Operation::Mount(MountKind::Tmpfs) => self.mounts.start_mount_tmpfs(&path),
                    Operation::Mount(MountKind::Hugepages) => {
                        self.mounts.start_mount_hugepages(&path)
                    }
                    Operation::Unmount => self.mounts.start_unmount_memory(&path),
                };
                match request {
                    Ok(request) => self.stage = Stage::Request(request),
                    Err(error) => return Some(Err(filesystem_error(error))),
                }
            }
            Stage::Request(request) => {
                let result = completion(request.try_complete())?;
                return Some(result.map(|()| 0).map_err(filesystem_error));
            }
        }
        None
    }
}

fn mount_target_allowed(path: &[u8]) -> bool {
    USER_MOUNT_ROOTS
        .iter()
        .any(|root| path == *root || path.starts_with(root) && path.get(root.len()) == Some(&b'/'))
}

pub(super) fn check_privileged(process: &Process<'_>) -> Result<(), Errno> {
    // Every guest starts as uid 0, so root credentials alone cannot distinguish an
    // ordinary program from one that init or a test granted mount permission.
    if process.mount && process.credentials.uid == 0 && process.credentials.euid == 0 {
        Ok(())
    } else {
        Err(EPERM)
    }
}

fn optional_string(process: &Process<'_>, address: usize) -> Result<Vec<u8>, Errno> {
    if address == 0 {
        Ok(Vec::new())
    } else {
        open::path_bytes(process, address)
    }
}
