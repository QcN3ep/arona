// SPDX-License-Identifier: MPL-2.0

//! Linux 目录遍历与共享游标。

use alloc::rc::Rc;
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
}

pub(in crate::syscall) fn read<'a>(
    process: &Process<'a>,
    raw: usize,
    address: usize,
    length: usize,
) -> Outcome<'a> {
    let operation = (|| {
        let description = Rc::clone(&process.files.get(fd(raw)?).ok_or(EBADF)?.description);
        if !matches!(description.as_ref(), OpenFileDescription::Directory { .. }) {
            return Err(ENOTDIR);
        }
        Ok(Pending::Directory(ReadDirectory {
            description,
            position: None,
            request: None,
            address,
            length: (length as u32 as usize).min(PAGE_SIZE),
        }))
    })();
    outcome(operation)
}

impl ReadDirectory<'_> {
    pub fn call(&self) -> Option<&Handle<Call>> {
        self.request
            .as_ref()
            .map(fs::client::PendingDirectoryPage::call)
    }

    pub fn poll(&mut self, process: &Process<'_>) -> Option<SyscallResult> {
        if self.position.is_none() {
            self.position = Some(self.description.position()?);
        }
        let position = self.position.as_ref().unwrap();
        if self.request.is_none() {
            let OpenFileDescription::Directory { session, .. } = self.description.as_ref() else {
                unreachable!()
            };
            match session.start_read_dir_all(position.get()) {
                Ok(request) => self.request = Some(request),
                Err(error) => return Some(Err(filesystem_error(error))),
            }
        }
        let entries = match completion(self.request.as_mut().unwrap().try_complete())? {
            Ok(entries) => entries,
            Err(error) => return Some(Err(filesystem_error(error))),
        };
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
            bytes[18] = match entry.kind {
                fs::FileType::Unknown => 0,
                fs::FileType::Regular => 8,
                fs::FileType::Directory => 4,
                fs::FileType::SymbolicLink => 10,
            };
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
}
