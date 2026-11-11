// SPDX-License-Identifier: MPL-2.0

//! Linux working directory, links, and naming operations.

// TODO: Support chmod, chown, access, and their relative directory operations.

use alloc::{rc::Rc, vec::Vec};
use core::cell::Cell;
use runtime::{Call, Handle};

use super::super::{
    Outcome,
    abi::{self, EBADF, EFAULT, EINVAL, ENOTDIR, EOPNOTSUPP, ERANGE, Errno, SyscallResult},
};
use super::{Pending, completion, fd, filesystem_error, open, outcome};
use crate::{
    file::{Descriptor, OpenFileDescription},
    process::Process,
};

pub(in crate::syscall) struct Lookup<'a> {
    pub name: Vec<u8>,
    pub anchor: Option<Rc<OpenFileDescription<'a>>>,
}

impl<'a> Lookup<'a> {
    pub fn new(process: &Process<'a>, directory: usize, address: usize) -> Result<Self, Errno> {
        Self::from_name(process, directory, open::path(process, address)?)
    }

    pub fn from_name(
        process: &Process<'a>,
        directory: usize,
        name: Vec<u8>,
    ) -> Result<Self, Errno> {
        let anchor = if name.starts_with(b"/") {
            None
        } else if directory as i32 == -100 {
            process.cwd.clone()
        } else {
            let description =
                Rc::clone(&process.files.get(fd(directory)?).ok_or(EBADF)?.description);
            if !matches!(description.as_ref(), OpenFileDescription::Directory { .. }) {
                return Err(ENOTDIR);
            }
            Some(description)
        };
        Ok(Self { name, anchor })
    }

    pub fn directory(&self) -> Option<&fs::Directory<'a>> {
        self.anchor
            .as_ref()
            .and_then(|anchor| match anchor.as_ref() {
                OpenFileDescription::Directory { session, .. } => Some(session),
                _ => None,
            })
    }
}

pub(in crate::syscall) struct PathOperation<'a> {
    request: Request<'a>,
    _anchors: Vec<Rc<OpenFileDescription<'a>>>,
}

enum Request<'a> {
    Mutation(fs::client::Pending<()>),
    ChangeDirectory(fs::client::PendingNode<'a>),
    ReadLink {
        request: fs::client::PendingRead,
        address: usize,
        length: usize,
    },
    GetCwd {
        request: fs::client::PendingRead,
        address: usize,
        length: usize,
    },
}

pub(super) fn dispatch<'a>(
    process: &mut Process<'a>,
    files: &fs::Client<'a>,
    number: usize,
    args: [usize; 6],
) -> Option<Outcome<'a>> {
    Some(match number {
        abi::FCHDIR => Outcome::Complete((|| {
            let anchor = Rc::clone(&process.files.get(fd(args[0])?).ok_or(EBADF)?.description);
            if !matches!(anchor.as_ref(), OpenFileDescription::Directory { .. }) {
                return Err(ENOTDIR);
            }
            process.cwd = Some(anchor);
            Ok(0)
        })()),
        abi::GETCWD => getcwd(process, args[0], args[1]),
        abi::CHDIR
        | abi::MKDIRAT
        | abi::UNLINKAT
        | abi::READLINKAT
        | abi::SYMLINKAT
        | abi::LINKAT
        | abi::RENAMEAT
        | abi::RENAMEAT2 => outcome(start(process, files, number, args)),
        _ => return None,
    })
}

fn start<'a>(
    process: &Process<'a>,
    files: &fs::Client<'a>,
    number: usize,
    args: [usize; 6],
) -> Result<Pending<'a>, Errno> {
    let (directory, name) = match number {
        abi::CHDIR => ((-100i32) as usize, args[0]),
        abi::SYMLINKAT => (args[1], args[2]),
        _ => (args[0], args[1]),
    };
    let lookup = Lookup::new(process, directory, name)?;
    if number == abi::UNLINKAT {
        check_remove(&lookup.name, args[2])?;
    }
    let mut anchors = Vec::new();
    let request = match number {
        abi::CHDIR => Request::ChangeDirectory(
            files
                .start_open_at(
                    lookup.directory(),
                    &lookup.name,
                    fs::OpenOptions {
                        write: false,
                        create: false,
                        exclusive: false,
                        truncate: false,
                        permissions: 0,
                    },
                    true,
                    true,
                )
                .map_err(filesystem_error)?,
        ),
        abi::MKDIRAT => Request::Mutation(
            files
                .start_mkdir_at(
                    lookup.directory(),
                    &lookup.name,
                    args[2] as u16 & 0o7777 & !process.umask,
                )
                .map_err(filesystem_error)?,
        ),
        abi::UNLINKAT => {
            if args[2] as u32 & !0x200 != 0 {
                return Err(EINVAL);
            }
            Request::Mutation(
                files
                    .start_unlink_at(lookup.directory(), &lookup.name, args[2] & 0x200 != 0)
                    .map_err(filesystem_error)?,
            )
        }
        abi::READLINKAT => read_link(files, &lookup, args[2], args[3])?,
        abi::SYMLINKAT => {
            let target = open::path_bytes(process, args[0])?;
            if target.is_empty() {
                return Err(abi::ENOENT);
            }
            Request::Mutation(
                files
                    .start_symlink_at(&target, lookup.directory(), &lookup.name)
                    .map_err(filesystem_error)?,
            )
        }
        abi::RENAMEAT | abi::RENAMEAT2 | abi::LINKAT => {
            let (pending, anchor) = pair(process, files, number, args, &lookup)?;
            anchors.extend(anchor);
            Request::Mutation(pending)
        }
        _ => unreachable!(),
    };
    anchors.extend(lookup.anchor);
    Ok(Pending::Path(PathOperation {
        request,
        _anchors: anchors,
    }))
}

fn getcwd<'a>(process: &Process<'a>, address: usize, length: usize) -> Outcome<'a> {
    if length == 0 {
        return Outcome::Complete(Err(EINVAL));
    }
    let Some(anchor) = process.cwd.as_ref() else {
        return Outcome::Complete(write_cwd(process, address, length, b"/"));
    };
    let OpenFileDescription::Directory { session, .. } = anchor.as_ref() else {
        unreachable!()
    };
    outcome(
        session
            .start_path()
            .map(|request| {
                Pending::Path(PathOperation {
                    request: Request::GetCwd {
                        request,
                        address,
                        length,
                    },
                    _anchors: alloc::vec![Rc::clone(anchor)],
                })
            })
            .map_err(filesystem_error),
    )
}

impl<'a> PathOperation<'a> {
    pub fn call(&self) -> &Handle<Call> {
        match &self.request {
            Request::Mutation(request) => request.call(),
            Request::ChangeDirectory(request) => request.call(),
            Request::ReadLink { request, .. } | Request::GetCwd { request, .. } => request.call(),
        }
    }

    pub fn poll(&mut self, process: &mut Process<'a>) -> Option<SyscallResult> {
        Some(match &mut self.request {
            Request::Mutation(request) => completion(request.try_complete())?
                .map(|()| 0)
                .map_err(filesystem_error),
            Request::ChangeDirectory(request) => match completion(request.try_complete())? {
                Ok(fs::client::OpenedNode::Directory(session)) => {
                    process.cwd = Some(directory_descriptor(session, 0, false).description);
                    Ok(0)
                }
                Ok(fs::client::OpenedNode::File(_)) => Err(ENOTDIR),
                Err(error) => Err(filesystem_error(error)),
            },
            Request::ReadLink {
                request,
                address,
                length,
            } => match completion(request.try_complete())? {
                Ok(bytes) => {
                    let count = bytes.len().min(*length);
                    process
                        .address_space
                        .write(*address, &bytes[..count])
                        .map(|()| count)
                        .map_err(|_| EFAULT)
                }
                Err(error) => Err(filesystem_error(error)),
            },
            Request::GetCwd {
                request,
                address,
                length,
            } => match completion(request.try_complete())? {
                Ok(bytes) => write_cwd(process, *address, *length, &bytes),
                Err(error) => Err(filesystem_error(error)),
            },
        })
    }
}

pub(super) fn directory_descriptor(
    session: fs::Directory<'_>,
    flags: u32,
    close_on_exec: bool,
) -> Descriptor<'_> {
    Descriptor {
        description: Rc::new(OpenFileDescription::Directory {
            session,
            flags: Cell::new(flags),
            offset: Cell::new(0),
            busy: Cell::new(false),
        }),
        close_on_exec,
    }
}

fn write_cwd(process: &Process<'_>, address: usize, length: usize, name: &[u8]) -> SyscallResult {
    if length <= name.len() {
        return Err(ERANGE);
    }
    let mut bytes = name.to_vec();
    bytes.push(0);
    process
        .address_space
        .write(address, &bytes)
        .map_err(|_| EFAULT)?;
    Ok(bytes.len())
}

fn pair<'a>(
    process: &Process<'a>,
    files: &fs::Client<'a>,
    number: usize,
    args: [usize; 6],
    lookup: &Lookup<'a>,
) -> Result<(fs::client::Pending<()>, Option<Rc<OpenFileDescription<'a>>>), Errno> {
    let flags = if number == abi::RENAMEAT {
        0
    } else {
        args[4] as u32
    };
    if flags & !(if number == abi::LINKAT { 0x400 } else { 0 }) != 0 {
        return Err(EOPNOTSUPP);
    }
    let target = Lookup::new(process, args[2], args[3])?;
    let source_path = (lookup.directory(), lookup.name.as_slice());
    let target_path = (target.directory(), target.name.as_slice());
    let pending = if number == abi::LINKAT {
        files.start_hard_link_at(source_path, target_path, flags & 0x400 != 0)
    } else {
        files.start_rename_at(source_path, target_path)
    }
    .map_err(filesystem_error)?;
    Ok((pending, target.anchor))
}

fn check_remove(name: &[u8], flags: usize) -> Result<(), Errno> {
    match name
        .rsplit(|byte| *byte == b'/')
        .find(|part| !part.is_empty())
    {
        Some(b".") if flags & 0x200 != 0 => return Err(EINVAL),
        Some(b"..") if flags & 0x200 != 0 => return Err(abi::ENOTEMPTY),
        Some(b"." | b"..") => return Err(abi::EISDIR),
        None => return Err(abi::EBUSY),
        _ => {}
    }
    Ok(())
}

fn read_link<'a>(
    files: &fs::Client<'a>,
    lookup: &Lookup<'_>,
    address: usize,
    length: usize,
) -> Result<Request<'a>, Errno> {
    if length as isize <= 0 {
        return Err(EINVAL);
    }
    Ok(Request::ReadLink {
        request: files
            .start_read_link_at(lookup.directory(), &lookup.name)
            .map_err(filesystem_error)?,
        address,
        length,
    })
}
