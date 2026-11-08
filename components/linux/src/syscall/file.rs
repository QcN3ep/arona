// SPDX-License-Identifier: MPL-2.0

use super::{
    Operation, Outcome,
    abi::{EBADF, EFAULT, EINVAL, EIO, SyscallResult},
};
use crate::process::Process;

pub(super) fn write(process: &Process, fd: usize, address: usize, length: usize) -> Outcome {
    let Some(file) = process.files.get(&fd) else {
        return Outcome::Complete(Err(EBADF));
    };
    if length > isize::MAX as usize {
        return Outcome::Complete(Err(EINVAL));
    }
    if length == 0 {
        return Outcome::Complete(Ok(0));
    }
    let mut buffer = [0; devmgr::uart::MAX_BYTES];
    let buffer = &mut buffer[..length.min(devmgr::uart::MAX_BYTES)];
    if process.address_space.read(address, buffer).is_err() {
        return Outcome::Complete(Err(EFAULT));
    }
    match file.output.start_write(buffer) {
        Ok(request) => Outcome::Pending(Operation::Write(request)),
        Err(_) => Outcome::Complete(Err(EIO)),
    }
}

pub(super) fn poll_write(
    request: &mut devmgr::device::Pending<usize, usize>,
) -> Option<SyscallResult> {
    match request.try_complete() {
        Ok(length) => Some(Ok(length)),
        Err(devmgr::Error::Runtime(runtime::Error::Kernel(runtime::KernelError::WouldBlock))) => {
            None
        }
        Err(_) => Some(Err(EIO)),
    }
}
