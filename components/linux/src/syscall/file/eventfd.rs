// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Linux event counters with blocking reads and writes.

use super::{Buffer, buffer::IoVectors, fd};
use crate::{
    file::{Descriptor, O_NONBLOCK, OpenFileDescription},
    process::Process,
    syscall::{
        Operation, Outcome,
        abi::{EAGAIN, EBADF, EFAULT, EINVAL, EMFILE, SyscallResult},
    },
};
use alloc::rc::Rc;

const SEMAPHORE: u32 = 1;
const CLOEXEC: u32 = 0x80000;

// REFERENCE: https://man7.org/linux/man-pages/man2/eventfd.2.html
pub(in crate::syscall) fn create(
    process: &mut Process<'_>,
    initial: u32,
    flags: usize,
) -> SyscallResult {
    let flags = flags as u32;
    if flags & !(SEMAPHORE | O_NONBLOCK | CLOEXEC) != 0 {
        return Err(EINVAL);
    }
    process
        .files
        .insert(Descriptor::eventfd(initial, flags, flags & CLOEXEC != 0), 0)
        .ok_or(EMFILE)
}

pub(in crate::syscall) enum Pending<'a> {
    Read {
        description: Rc<OpenFileDescription<'a>>,
        buffers: IoVectors,
    },
    Write {
        description: Rc<OpenFileDescription<'a>>,
        value: u64,
    },
}

pub(in crate::syscall) fn read<'a>(
    process: &Process<'a>,
    raw: usize,
    buffer: Buffer,
) -> Outcome<'a> {
    let result = (|| {
        let descriptor = process.files.get(fd(raw)?).ok_or(EBADF)?;
        let buffers = buffer.import(process)?;
        if buffers.length() < 8 {
            return Err(EINVAL);
        }
        Ok(Pending::Read {
            description: Rc::clone(&descriptor.description),
            buffers,
        })
    })();
    finish(process, result)
}

pub(in crate::syscall) fn write<'a>(
    process: &Process<'a>,
    raw: usize,
    buffer: Buffer,
) -> Outcome<'a> {
    let result = (|| {
        let descriptor = process.files.get(fd(raw)?).ok_or(EBADF)?;
        let buffers = buffer.import(process)?;
        if buffers.length() < 8 {
            return Err(EINVAL);
        }
        let bytes = buffers.gather(process, 8)?;
        let value = u64::from_le_bytes(bytes.as_slice().try_into().map_err(|_| EFAULT)?);
        if value == u64::MAX {
            return Err(EINVAL);
        }
        Ok(Pending::Write {
            description: Rc::clone(&descriptor.description),
            value,
        })
    })();
    finish(process, result)
}

fn finish<'a>(process: &Process<'a>, pending: Result<Pending<'a>, super::Errno>) -> Outcome<'a> {
    let pending = match pending {
        Ok(pending) => pending,
        Err(error) => return Outcome::Complete(Err(error)),
    };
    if let Some(result) = pending.poll(process) {
        return Outcome::Complete(result);
    }
    let (Pending::Read { description, .. } | Pending::Write { description, .. }) = &pending;
    if description.flags() & O_NONBLOCK != 0 {
        Outcome::Complete(Err(EAGAIN))
    } else {
        Outcome::Pending(Operation::File(super::Pending::Eventfd(pending)))
    }
}

impl Pending<'_> {
    pub fn poll(&self, process: &Process<'_>) -> Option<SyscallResult> {
        let (Self::Read { description, .. } | Self::Write { description, .. }) = self;
        let OpenFileDescription::Eventfd {
            counter, semaphore, ..
        } = description.as_ref()
        else {
            unreachable!()
        };
        let previous = counter.get();
        match self {
            Self::Read { buffers, .. } => {
                if previous == 0 {
                    return None;
                }
                let value = if *semaphore { 1u64 } else { previous };
                let result = buffers.scatter(process, &mut value.to_le_bytes());
                Some(match result {
                    Ok(8) => {
                        counter.set(previous - value);
                        Ok(8)
                    }
                    Ok(_) => Err(EFAULT),
                    Err(error) => Err(error),
                })
            }
            Self::Write { value, .. } => {
                let total = previous
                    .checked_add(*value)
                    .filter(|total| *total < u64::MAX)?;
                counter.set(total);
                Some(Ok(8))
            }
        }
    }
}
