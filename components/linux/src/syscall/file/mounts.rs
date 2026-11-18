// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Snapshot the native volumes and Linux-local mounts for procfs.

use super::{completion, filesystem_error};
use crate::{
    file::Descriptor,
    process::Process,
    syscall::abi::{EMFILE, SyscallResult},
    vfs::mount::{self, Backend, MOUNTS},
};
use alloc::vec::Vec;
use runtime::{Call, Handle};

pub(in crate::syscall) struct Read {
    request: fs::client::PendingVolume,
    index: usize,
    bytes: Vec<u8>,
    inode: u64,
    flags: u32,
}

impl Read {
    pub fn new(
        files: &fs::Client<'_>,
        inode: u64,
        flags: u32,
    ) -> Result<Self, crate::syscall::abi::Errno> {
        Ok(Self {
            request: files.start_volume(0).map_err(filesystem_error)?,
            index: 0,
            bytes: Vec::new(),
            inode,
            flags,
        })
    }
    pub fn call(&self) -> &Handle<Call> {
        self.request.call()
    }
    pub fn poll(
        &mut self,
        process: &mut Process<'_>,
        files: &fs::Client<'_>,
    ) -> Option<SyscallResult> {
        match completion(self.request.try_complete())? {
            Ok(Some(volume)) => {
                mount::append_volume(&mut self.bytes, &volume);
                self.index += 1;
                match files.start_volume(self.index) {
                    Ok(request) => self.request = request,
                    Err(error) => return Some(Err(filesystem_error(error))),
                }
                None
            }
            Ok(None) => {
                for mount in MOUNTS
                    .iter()
                    .filter(|mount| !matches!(mount.backend, Backend::Native))
                {
                    mount::line(
                        &mut self.bytes,
                        mount.name.as_bytes(),
                        mount.path,
                        mount.name,
                        mount.readonly,
                    );
                }
                let descriptor = Descriptor::snapshot(
                    core::mem::take(&mut self.bytes),
                    self.inode,
                    self.flags & !super::O_CLOEXEC,
                    self.flags & super::O_CLOEXEC != 0,
                );
                Some(process.files.insert(descriptor, 0).ok_or(EMFILE))
            }
            Err(error) => Some(Err(filesystem_error(error))),
        }
    }
}
