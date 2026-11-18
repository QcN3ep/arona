// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Resolve pathname arguments before starting a filesystem operation.

use super::{Pending, open, path::Lookup};
use crate::{
    Services,
    process::Process,
    syscall::{
        Operation as SyscallOperation, Outcome,
        abi::{self, EINVAL, Errno, SyscallResult},
    },
    vfs::{Resolved, Walk},
};
use alloc::{boxed::Box, collections::VecDeque, vec::Vec};
use runtime::{Call, Handle};

pub(in crate::syscall) struct Prepared<'a>(VecDeque<Resolved<'a>>);

impl<'a> Prepared<'a> {
    pub fn empty() -> Self {
        Self(VecDeque::new())
    }
    pub fn take(&mut self) -> Result<Resolved<'a>, Errno> {
        self.0.pop_front().ok_or(EINVAL)
    }
    pub fn first(&self) -> Option<&Resolved<'a>> {
        self.0.front()
    }
}

pub(in crate::syscall) struct Resolve<'a> {
    number: usize,
    arguments: [usize; 6],
    walks: VecDeque<Walk<'a>>,
    prepared: Prepared<'a>,
    inner: Option<Box<Pending<'a>>>,
    node: Option<fs::Metadata>,
}

pub(super) fn start<'a>(
    process: &Process<'a>,
    number: usize,
    args: [usize; 6],
) -> Option<Outcome<'a>> {
    if matches!(number, abi::MOUNT | abi::UMOUNT2)
        && let Err(error) = super::mount::check_privileged(process)
    {
        return Some(Outcome::Complete(Err(error)));
    }
    let flags = if number == abi::OPENAT {
        match open::validate_flags(args[2]) {
            Ok(flags) => flags,
            Err(error) => return Some(Outcome::Complete(Err(error))),
        }
    } else {
        0
    };
    let at = crate::syscall::AT_FDCWD;
    let specifications: Vec<(usize, usize, bool, bool)> = match number {
        abi::OPENAT => alloc::vec![(
            args[0],
            args[1],
            flags & 0x20000 == 0 && flags & 0xc0 != 0xc0,
            flags & 0x40 != 0
        )],
        abi::CHDIR => alloc::vec![(at, args[0], true, false)],
        abi::MKDIRAT => alloc::vec![(args[0], args[1], false, true)],
        abi::UNLINKAT | abi::READLINKAT => alloc::vec![(args[0], args[1], false, false)],
        abi::SYMLINKAT => alloc::vec![(args[1], args[2], false, true)],
        abi::RENAMEAT | abi::RENAMEAT2 | abi::LINKAT => alloc::vec![
            (
                args[0],
                args[1],
                number == abi::LINKAT && args[4] & 0x400 != 0,
                false
            ),
            (args[2], args[3], false, true)
        ],
        abi::NEWFSTATAT | abi::STATX => alloc::vec![(
            args[0],
            args[1],
            args[if number == abi::STATX { 2 } else { 3 }] & 0x100 == 0,
            false
        )],
        abi::FACCESSAT | abi::FACCESSAT2 => alloc::vec![(
            args[0],
            args[1],
            number == abi::FACCESSAT || args[3] & 0x100 == 0,
            false
        )],
        abi::UTIMENSAT => alloc::vec![(args[0], args[1], args[3] & 0x100 == 0, false)],
        abi::STATFS => alloc::vec![(at, args[0], true, false)],
        abi::MOUNT => alloc::vec![(at, args[1], true, false)],
        abi::UMOUNT2 => alloc::vec![(at, args[0], true, false)],
        _ => return None,
    };
    let operation = (|| {
        let mut walks = VecDeque::new();
        for (directory, address, follow, missing) in specifications {
            if address == 0 && matches!(number, abi::NEWFSTATAT | abi::STATX) {
                continue;
            }
            let mut name = open::path_bytes(process, address)?;
            if name.is_empty() {
                if !matches!(
                    number,
                    abi::READLINKAT
                        | abi::NEWFSTATAT
                        | abi::STATX
                        | abi::FACCESSAT
                        | abi::FACCESSAT2
                        | abi::UTIMENSAT
                ) {
                    return Err(abi::ENOENT);
                }
                continue;
            }
            if number == abi::MKDIRAT {
                while name.len() > 1 && name.last() == Some(&b'/') {
                    name.pop();
                }
            }
            let lookup = Lookup::from_name(process, directory, name)?;
            walks.push_back(Walk::new(lookup, follow, missing)?);
        }
        Ok(Resolve {
            number,
            arguments: args,
            walks,
            prepared: Prepared::empty(),
            inner: None,
            node: None,
        })
    })();
    Some(match operation {
        Ok(resolve) => super::outcome(Ok(Pending::Resolve(Box::new(resolve)))),
        Err(error) => Outcome::Complete(Err(error)),
    })
}

impl<'a> Resolve<'a> {
    pub fn call(&self) -> Option<&Handle<Call>> {
        self.walks.front().and_then(Walk::call)
    }
    pub fn waits<'b>(&'b self, items: &mut Vec<runtime::ipc::WaitItem<'b>>) {
        if let Some(inner) = self.inner.as_ref() {
            inner.waits(items);
        }
    }
    pub fn interrupt(&self, process: &Process<'_>) -> Option<(bool, SyscallResult)> {
        self.inner
            .as_ref()
            .and_then(|inner| inner.interrupt(process))
    }
    pub fn poll(
        &mut self,
        process: &mut Process<'a>,
        index: usize,
        files: &fs::Client<'a>,
        services: &Services<'a>,
    ) -> Option<SyscallResult> {
        let result = self.poll_inner(process, index, files, services)?;
        if let (Ok(fd), Some(mut info)) = (result, self.node)
            && let Some(descriptor) = process.files.get_mut(fd)
        {
            if let Some(crate::file::OpenFileDescription::VirtualFile { metadata, .. }) =
                alloc::rc::Rc::get_mut(&mut descriptor.description)
            {
                info.size = metadata.size;
                info.blocks = metadata.blocks;
                *metadata = info;
            }
            descriptor.node = Some(info);
        }
        Some(result)
    }
    fn poll_inner(
        &mut self,
        process: &mut Process<'a>,
        index: usize,
        files: &fs::Client<'a>,
        services: &Services<'a>,
    ) -> Option<SyscallResult> {
        if let Some(inner) = self.inner.as_mut() {
            return inner.poll(process, index, files, services);
        }
        while let Some(walk) = self.walks.front_mut() {
            match walk.poll(process, files, services)? {
                Ok(resolved) => {
                    if self.number == abi::OPENAT && resolved.local {
                        self.node = resolved.info;
                    }
                    self.prepared.0.push_back(resolved);
                }
                Err(error) => return Some(Err(error)),
            }
            self.walks.pop_front();
        }
        match super::dispatch_resolved(
            process,
            index,
            files,
            services,
            self.number,
            self.arguments,
            &mut self.prepared,
        )? {
            Outcome::Complete(result) => Some(result),
            Outcome::Pending(SyscallOperation::File(pending)) => {
                self.inner = Some(Box::new(pending));
                self.inner
                    .as_mut()
                    .unwrap()
                    .poll(process, index, files, services)
            }
            _ => unreachable!(),
        }
    }
}
