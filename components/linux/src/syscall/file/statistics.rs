// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Filesystem statistics for resolved paths and open descriptions.

use alloc::rc::Rc;
use runtime::{Call, Handle};

use super::{Pending, completion, fd, filesystem_error, outcome};
use crate::{
    file::OpenFileDescription,
    process::Process,
    syscall::{
        Outcome,
        abi::{EBADF, EFAULT, EOPNOTSUPP, SyscallResult},
    },
};

pub(in crate::syscall) struct Statistics<'a> {
    request: Source,
    address: usize,
    _description: Option<Rc<OpenFileDescription<'a>>>,
}

enum Source {
    Native(fs::client::PendingStatistics),
    Local(fs::Statistics),
}

pub(super) fn start<'a>(
    process: &Process<'a>,
    files: &fs::Client<'a>,
    source: usize,
    address: usize,
    descriptor: bool,
    paths: &mut super::resolve::Prepared<'a>,
) -> Outcome<'a> {
    outcome((|| {
        let (request, description) = if descriptor {
            let description = Rc::clone(&process.files.get(fd(source)?).ok_or(EBADF)?.description);
            let local_mount = process
                .files
                .get(fd(source)?)
                .and_then(|descriptor| descriptor.node)
                .and_then(|info| {
                    crate::vfs::mount::MOUNTS.iter().find(|mount| {
                        !matches!(mount.backend, crate::vfs::mount::Backend::Native)
                            && crate::vfs::mount::identity(mount.path) == info.device
                    })
                });
            let request = if let Some(mount) = local_mount {
                Source::Local(crate::vfs::mount::statistics(mount.path))
            } else {
                match description.as_ref() {
                    OpenFileDescription::Regular { file, .. } => {
                        Source::Native(file.start_statistics().map_err(filesystem_error)?)
                    }
                    OpenFileDescription::Directory { session, .. } => {
                        Source::Native(session.start_statistics().map_err(filesystem_error)?)
                    }
                    OpenFileDescription::VirtualDirectory { path, .. } => {
                        Source::Local(crate::vfs::mount::statistics(path))
                    }
                    OpenFileDescription::VirtualFile { metadata, .. } => Source::Local(
                        crate::vfs::mount::MOUNTS
                            .iter()
                            .find(|mount| {
                                crate::vfs::mount::identity(mount.path) == metadata.device
                            })
                            .map(|mount| crate::vfs::mount::statistics(mount.path))
                            .unwrap_or_else(|| crate::vfs::mount::statistics(b"/proc")),
                    ),
                    OpenFileDescription::Virtual { .. }
                    | OpenFileDescription::Rtc { .. }
                    | OpenFileDescription::Terminal { .. }
                    | OpenFileDescription::Block { .. } => {
                        Source::Local(crate::vfs::mount::statistics(b"/dev"))
                    }
                    OpenFileDescription::MessageQueue { .. }
                    | OpenFileDescription::MqueueFile { .. } => {
                        Source::Local(crate::vfs::mount::statistics(b"/dev/mqueue"))
                    }
                    _ => return Err(EOPNOTSUPP),
                }
            };
            (request, Some(description))
        } else {
            let resolved = paths.take()?;
            let lookup = resolved.lookup;
            let request = if resolved.local {
                Source::Local(crate::vfs::mount::statistics(&resolved.path))
            } else {
                Source::Native(
                    files
                        .start_statistics_at(lookup.directory(), &lookup.name)
                        .map_err(filesystem_error)?,
                )
            };
            (request, lookup.anchor)
        };
        Ok(Pending::Statistics(Statistics {
            request,
            address,
            _description: description,
        }))
    })())
}

impl Statistics<'_> {
    pub fn call(&self) -> Option<&Handle<Call>> {
        match &self.request {
            Source::Native(request) => Some(request.call()),
            Source::Local(_) => None,
        }
    }

    // REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/include/uapi/asm-generic/statfs.h?h=v6.12
    pub fn poll(&mut self, process: &Process<'_>) -> Option<SyscallResult> {
        Some(
            match &mut self.request {
                Source::Native(request) => {
                    completion(request.try_complete())?.map_err(filesystem_error)
                }
                Source::Local(statistics) => Ok(*statistics),
            }
            .and_then(|statistics| {
                let mut bytes = [0; 120];
                for (output, word) in bytes.chunks_exact_mut(8).zip([
                    statistics.kind,
                    statistics.block_size,
                    statistics.blocks,
                    statistics.free_blocks,
                    statistics.available_blocks,
                    statistics.files,
                    statistics.free_files,
                    statistics.device,
                    statistics.name_length,
                    statistics.block_size,
                    statistics.flags | 0x20,
                ]) {
                    output.copy_from_slice(&word.to_le_bytes());
                }
                process
                    .address_space
                    .write(self.address, &bytes)
                    .map(|()| 0)
                    .map_err(|_| EFAULT)
            }),
        )
    }
}
