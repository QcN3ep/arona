// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Filesystem statistics for resolved paths and open descriptions.

use alloc::rc::Rc;
use runtime::{Call, Handle};

use super::{Pending, completion, fd, filesystem_error, outcome, path::Lookup};
use crate::{
    file::OpenFileDescription,
    process::Process,
    syscall::{
        Outcome,
        abi::{EBADF, EFAULT, EOPNOTSUPP, SyscallResult},
    },
};

pub(in crate::syscall) struct Statistics<'a> {
    request: fs::client::PendingStatistics,
    address: usize,
    _description: Option<Rc<OpenFileDescription<'a>>>,
}

pub(super) fn start<'a>(
    process: &Process<'a>,
    files: &fs::Client<'a>,
    source: usize,
    address: usize,
    descriptor: bool,
) -> Outcome<'a> {
    outcome((|| {
        let (request, description) = if descriptor {
            let description = Rc::clone(&process.files.get(fd(source)?).ok_or(EBADF)?.description);
            let request = match description.as_ref() {
                OpenFileDescription::Regular { file, .. } => file.start_statistics(),
                OpenFileDescription::Directory { session, .. } => session.start_statistics(),
                _ => return Err(EOPNOTSUPP),
            }
            .map_err(filesystem_error)?;
            (request, Some(description))
        } else {
            let lookup = Lookup::new(process, (-100i32) as usize, source)?;
            let request = files
                .start_statistics_at(lookup.directory(), &lookup.name)
                .map_err(filesystem_error)?;
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
    pub fn call(&self) -> &Handle<Call> {
        self.request.call()
    }

    // REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/include/uapi/asm-generic/statfs.h?h=v6.12
    pub fn poll(&mut self, process: &Process<'_>) -> Option<SyscallResult> {
        Some(
            completion(self.request.try_complete())?
                .map_err(filesystem_error)
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
