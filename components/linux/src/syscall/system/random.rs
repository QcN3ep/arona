// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Entropy source reads and userspace buffer copying.

use super::{Pending as Operation, outcome};
use crate::{
    Services,
    process::Process,
    syscall::{
        Outcome,
        abi::{EFAULT, EINVAL, EIO, EOPNOTSUPP, SyscallResult},
    },
};
use runtime::{Call, Handle};

pub(in crate::syscall) struct Pending {
    request: devmgr::rng::PendingRead,
    address: usize,
}

// REFERENCE: https://man7.org/linux/man-pages/man2/getrandom.2.html
pub(super) fn start<'a>(
    _process: &Process<'a>,
    services: &Services<'_>,
    args: [usize; 6],
) -> Outcome<'a> {
    let [address, length, flags, ..] = args;
    if flags as u32 & !7 != 0 || flags & 6 == 6 {
        return Outcome::Complete(Err(EINVAL));
    }
    if length == 0 {
        return Outcome::Complete(Ok(0));
    }
    if length > isize::MAX as usize {
        return Outcome::Complete(Err(EINVAL));
    }
    outcome((|| {
        let entropy = services.entropy.as_ref().ok_or(EOPNOTSUPP)?;
        Ok(Operation::Random(Pending {
            request: entropy
                .start_read(length.min(devmgr::rng::MAX_BYTES))
                .map_err(|_| EIO)?,
            address,
        }))
    })())
}

impl Pending {
    pub fn call(&self) -> &Handle<Call> {
        self.request.call()
    }
    pub fn poll(&mut self, process: &Process<'_>) -> Option<SyscallResult> {
        let bytes = match self.request.try_complete() {
            Err(devmgr::Error::Runtime(runtime::Error::Kernel(
                runtime::KernelError::WouldBlock,
            ))) => return None,
            Err(_) => return Some(Err(EIO)),
            Ok(bytes) => bytes,
        };
        Some(
            process
                .address_space
                .write(self.address, &bytes)
                .map(|()| bytes.len())
                .map_err(|_| EFAULT),
        )
    }
}
