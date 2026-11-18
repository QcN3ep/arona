// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Linux pathname traversal over native and process-local filesystems.

mod devfs;
pub(crate) mod directory;
pub(crate) mod mount;
mod network;
pub(crate) mod procfs;
mod walk;
pub(crate) use directory::Entry;
pub(crate) mod local;

use crate::syscall::file::fd;
use crate::{
    file::OpenFileDescription,
    process::Process,
    syscall::abi::{EBADF, ENOTDIR, Errno},
};
use alloc::{rc::Rc, vec::Vec};

pub(crate) use walk::{Resolved, Walk};

pub(crate) struct Lookup<'a> {
    pub name: Vec<u8>,
    pub anchor: Option<Rc<OpenFileDescription<'a>>>,
}

impl<'a> Lookup<'a> {
    pub fn new(process: &Process<'a>, directory: usize, address: usize) -> Result<Self, Errno> {
        Self::from_name(
            process,
            directory,
            crate::syscall::file::open::path(process, address)?,
        )
    }

    pub fn from_name(
        process: &Process<'a>,
        directory: usize,
        name: Vec<u8>,
    ) -> Result<Self, Errno> {
        let anchor = if name.starts_with(b"/") {
            None
        } else if directory == crate::syscall::AT_FDCWD {
            process.cwd.clone()
        } else {
            let description =
                Rc::clone(&process.files.get(fd(directory)?).ok_or(EBADF)?.description);
            if !matches!(
                description.as_ref(),
                OpenFileDescription::Directory { .. }
                    | OpenFileDescription::VirtualDirectory { .. }
            ) {
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
