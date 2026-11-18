// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Linux directory traversal and shared cursors.

use alloc::{collections::VecDeque, rc::Rc, vec::Vec};
use runtime::{Call, Handle};

use super::super::{
    Outcome,
    abi::{EBADF, EFAULT, EINVAL, ENOTDIR, SyscallResult},
};
use super::{Pending, completion, fd, filesystem_error, outcome};
use crate::{
    elf::PAGE_SIZE,
    file::{FilePosition, OpenFileDescription},
    process::Process,
};

pub(in crate::syscall) struct ReadDirectory<'a> {
    description: Rc<OpenFileDescription<'a>>,
    position: Option<FilePosition<'a>>,
    request: Option<fs::client::PendingDirectoryPage>,
    address: usize,
    length: usize,
    path: Option<Vec<u8>>,
    path_request: Option<fs::client::PendingRead>,
    overlay_request: Option<fs::client::PendingMetadata>,
    overlays: VecDeque<(Vec<u8>, Vec<u8>)>,
    gathered: Vec<crate::vfs::Entry>,
    loaded: bool,
    overlays_initialized: bool,
    volume: Option<fs::client::PendingVolume>,
    volume_index: usize,
    volumes_done: bool,
}

pub(in crate::syscall) fn read<'a>(
    process: &Process<'a>,
    raw: usize,
    address: usize,
    length: usize,
) -> Outcome<'a> {
    let operation = (|| {
        let description = Rc::clone(&process.files.get(fd(raw)?).ok_or(EBADF)?.description);
        if !matches!(
            description.as_ref(),
            OpenFileDescription::Directory { .. } | OpenFileDescription::VirtualDirectory { .. }
        ) {
            return Err(ENOTDIR);
        }
        if description.path_only() {
            return Err(EBADF);
        }
        Ok(Pending::Directory(alloc::boxed::Box::new(ReadDirectory {
            description,
            position: None,
            request: None,
            path: None,
            path_request: None,
            overlay_request: None,
            overlays: VecDeque::new(),
            gathered: Vec::new(),
            loaded: false,
            overlays_initialized: false,
            volume: None,
            volume_index: 0,
            volumes_done: false,
            address,
            length: (length as u32 as usize).min(PAGE_SIZE),
        })))
    })();
    outcome(operation)
}

impl ReadDirectory<'_> {
    pub fn call(&self) -> Option<&Handle<Call>> {
        self.request
            .as_ref()
            .map(fs::client::PendingDirectoryPage::call)
            .or_else(|| {
                self.path_request
                    .as_ref()
                    .map(fs::client::PendingRead::call)
            })
            .or_else(|| self.volume.as_ref().map(fs::client::PendingVolume::call))
            .or_else(|| {
                self.overlay_request
                    .as_ref()
                    .map(fs::client::PendingMetadata::call)
            })
    }

    pub fn poll(
        &mut self,
        process: &Process<'_>,
        files: &fs::Client<'_>,
        services: &crate::Services<'_>,
    ) -> Option<SyscallResult> {
        if self.position.is_none() {
            self.position = Some(self.description.position()?);
        }
        let description = Rc::clone(&self.description);
        let cache = match description.as_ref() {
            OpenFileDescription::Directory { entries, .. }
            | OpenFileDescription::VirtualDirectory { entries, .. } => entries,
            _ => return Some(Err(ENOTDIR)),
        };
        if cache.borrow().is_none() {
            match self.load(process, files, services)? {
                Ok(()) => {}
                Err(error) => return Some(Err(error)),
            }
            *cache.borrow_mut() = Some(core::mem::take(&mut self.gathered));
        }
        let position = self.position.as_ref().unwrap();
        let borrowed = cache.borrow();
        let entries = borrowed.as_ref().unwrap().iter().skip(position.get());
        let mut copied = 0;
        // REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/include/linux/dirent.h?h=v6.12
        for entry in entries {
            let length = (20 + entry.name.len()).next_multiple_of(8);
            if copied + length > self.length {
                return Some(if copied == 0 { Err(EINVAL) } else { Ok(copied) });
            }
            let Some(next) = position
                .get()
                .checked_add(1)
                .filter(|next| *next <= i64::MAX as usize)
            else {
                return Some(if copied == 0 { Err(EINVAL) } else { Ok(copied) });
            };
            let mut bytes = [0; 280];
            bytes[..8].copy_from_slice(&entry.inode.to_le_bytes());
            bytes[8..16].copy_from_slice(&(next as u64).to_le_bytes());
            bytes[16..18].copy_from_slice(&(length as u16).to_le_bytes());
            bytes[18] = entry.kind;
            bytes[19..19 + entry.name.len()].copy_from_slice(&entry.name);
            let result = self
                .address
                .checked_add(copied)
                .ok_or(EFAULT)
                .and_then(|address| {
                    process
                        .address_space
                        .write(address, &bytes[..length])
                        .map_err(|_| EFAULT)
                });
            if let Err(error) = result {
                return Some(if copied == 0 { Err(error) } else { Ok(copied) });
            }
            position.set(next);
            copied += length;
        }
        Some(Ok(copied))
    }
    fn load(
        &mut self,
        process: &Process<'_>,
        files: &fs::Client<'_>,
        services: &crate::Services<'_>,
    ) -> Option<Result<(), super::super::abi::Errno>> {
        if let Err(error) = self.load_entries(process, services)? {
            return Some(Err(error));
        }
        if let Err(error) = self.local_overlays(process, services) {
            return Some(Err(error));
        }
        if let Err(error) = self.load_volumes(files)? {
            return Some(Err(error));
        }
        if let Err(error) = self.load_parent(files)? {
            return Some(Err(error));
        }
        self.gathered.sort_unstable_by(|a, b| {
            let order = |name: &[u8]| match name {
                b"." => 0,
                b".." => 1,
                _ => 2,
            };
            order(&a.name)
                .cmp(&order(&b.name))
                .then_with(|| a.name.cmp(&b.name))
        });
        Some(Ok(()))
    }

    fn load_entries(
        &mut self,
        process: &Process<'_>,
        services: &crate::Services<'_>,
    ) -> Option<Result<(), super::super::abi::Errno>> {
        match self.description.as_ref() {
            OpenFileDescription::Directory { session, .. } => {
                if self.path.is_none() {
                    if self.path_request.is_none() {
                        match session.start_path() {
                            Ok(request) => self.path_request = Some(request),
                            Err(error) => return Some(Err(filesystem_error(error))),
                        }
                    }
                    match completion(self.path_request.as_mut().unwrap().try_complete())? {
                        Ok(path) => {
                            self.path = Some(path);
                            self.path_request = None;
                        }
                        Err(error) => return Some(Err(filesystem_error(error))),
                    }
                }
                while !self.loaded {
                    if self.request.is_none() {
                        match session.start_read_dir_all(self.gathered.len()) {
                            Ok(request) => self.request = Some(request),
                            Err(error) => return Some(Err(filesystem_error(error))),
                        }
                    }
                    let entries = match completion(self.request.as_mut().unwrap().try_complete())? {
                        Ok(entries) => entries,
                        Err(error) => return Some(Err(filesystem_error(error))),
                    };
                    self.request = None;
                    if entries.is_empty() {
                        self.loaded = true;
                        break;
                    }
                    self.gathered
                        .extend(entries.into_iter().map(crate::vfs::Entry::from));
                }
            }
            OpenFileDescription::VirtualDirectory { path, .. } if !self.loaded => {
                self.path = Some(path.clone());
                self.gathered = match crate::vfs::directory::entries(process, services, path) {
                    Ok(entries) => entries,
                    Err(error) => return Some(Err(error)),
                };
                self.loaded = true;
            }
            _ => {}
        }
        Some(Ok(()))
    }

    fn local_overlays(
        &mut self,
        process: &Process<'_>,
        services: &crate::Services<'_>,
    ) -> Result<(), super::super::abi::Errno> {
        let path = self.path.as_ref().unwrap();
        if !self.overlays_initialized {
            for mount in crate::vfs::mount::MOUNTS
                .iter()
                .filter(|mount| !matches!(mount.backend, crate::vfs::mount::Backend::Native))
            {
                let (parent, name) = crate::vfs::mount::split(mount.path);
                if parent == path {
                    let info = crate::vfs::local::metadata(process, services, mount.path)?;
                    crate::vfs::directory::overlay(&mut self.gathered, name, info);
                }
            }
            if crate::vfs::mount::MOUNTS
                .iter()
                .any(|mount| mount.path == path)
                && path != b"/"
            {
                let (parent, _) = crate::vfs::mount::split(path);
                if crate::vfs::local::contains(parent) {
                    let info = crate::vfs::local::metadata(process, services, parent)?;
                    crate::vfs::directory::overlay(&mut self.gathered, b"..", info);
                } else {
                    self.overlays.push_back((b"..".to_vec(), parent.to_vec()));
                }
            }
            self.overlays_initialized = true;
        }
        Ok(())
    }

    fn load_volumes(
        &mut self,
        files: &fs::Client<'_>,
    ) -> Option<Result<(), super::super::abi::Errno>> {
        let path = self.path.as_ref().unwrap();
        while !self.volumes_done {
            if self.volume.is_none() {
                match files.start_volume(self.volume_index) {
                    Ok(request) => self.volume = Some(request),
                    Err(error) => return Some(Err(filesystem_error(error))),
                }
            }
            match completion(self.volume.as_mut().unwrap().try_complete())? {
                Ok(Some(volume)) => {
                    let (parent, name) = crate::vfs::mount::split(&volume.path);
                    if parent == path
                        && matches!(
                            crate::vfs::mount::at(&volume.path).backend,
                            crate::vfs::mount::Backend::Native
                        )
                    {
                        crate::vfs::directory::overlay(&mut self.gathered, name, volume.root);
                    }
                }
                Ok(None) => self.volumes_done = true,
                Err(error) => return Some(Err(filesystem_error(error))),
            }
            self.volume = None;
            self.volume_index += 1;
        }
        Some(Ok(()))
    }

    fn load_parent(
        &mut self,
        files: &fs::Client<'_>,
    ) -> Option<Result<(), super::super::abi::Errno>> {
        while let Some((name, full)) = self.overlays.front() {
            if self.overlay_request.is_none() {
                match files.start_metadata_at(None, full, false) {
                    Ok(request) => self.overlay_request = Some(request),
                    Err(error) => return Some(Err(filesystem_error(error))),
                }
            }
            match completion(self.overlay_request.as_mut().unwrap().try_complete())? {
                Ok(info) => crate::vfs::directory::overlay(&mut self.gathered, name, info),
                Err(fs::Error::NotFound) => {}
                Err(error) => return Some(Err(filesystem_error(error))),
            }
            self.overlay_request = None;
            self.overlays.pop_front();
        }
        Some(Ok(()))
    }
}
