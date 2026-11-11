// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Signal descriptors, thread-local readiness, and signal information records.

use super::{Buffer, Pending, fd};
use crate::{
    file::{Descriptor, O_NONBLOCK, OpenFileDescription},
    process::Process,
    syscall::{
        Operation, Outcome,
        abi::{EAGAIN, EBADF, EFAULT, EINVAL, EMFILE, SyscallResult},
    },
};
use alloc::rc::Rc;

const SFD_CLOEXEC: u32 = 0x80000;
const SFD_NONBLOCK: u32 = O_NONBLOCK;
const SIZE: usize = 128;

// REFERENCE: https://man7.org/linux/man-pages/man2/signalfd.2.html
pub(in crate::syscall) fn create<'a>(
    process: &mut Process<'a>,
    raw: usize,
    address: usize,
    length: usize,
    flags: usize,
) -> SyscallResult {
    if length != 8 {
        return Err(EINVAL);
    }
    let flags = flags as u32;
    if flags & !(SFD_NONBLOCK | SFD_CLOEXEC) != 0 {
        return Err(EINVAL);
    }
    let mut bytes = [0; 8];
    process
        .address_space
        .read(address, &mut bytes)
        .map_err(|_| EFAULT)?;
    let mut mask = u64::from_le_bytes(bytes);
    mask &= !(1 << (9 - 1)) & !(1 << (19 - 1));
    if raw as i32 != -1 {
        let index = fd(raw)?;
        let descriptor = process.files.get(index).ok_or(EBADF)?;
        let OpenFileDescription::Signalfd { mask: current, .. } = descriptor.description.as_ref()
        else {
            return Err(EINVAL);
        };
        current.set(mask);
        return Ok(index);
    }
    process
        .files
        .insert(
            Descriptor::signalfd(mask, flags & SFD_NONBLOCK, flags & SFD_CLOEXEC != 0),
            0,
        )
        .ok_or(EMFILE)
}

pub(in crate::syscall) fn read<'a>(
    process: &Process<'a>,
    raw: usize,
    buffer: Buffer,
) -> Outcome<'a> {
    let result = (|| {
        let descriptor = process.files.get(fd(raw)?).ok_or(EBADF)?;
        let buffers = buffer.import(process)?;
        if buffers.length() < SIZE {
            return Err(EINVAL);
        }
        Ok(Outcome::Pending(Operation::File(Pending::Signalfd(Read {
            description: Rc::clone(&descriptor.description),
            buffers,
        }))))
    })();
    result.unwrap_or_else(|error| Outcome::Complete(Err(error)))
}

pub(in crate::syscall) struct Read<'a> {
    description: Rc<OpenFileDescription<'a>>,
    buffers: super::buffer::IoVectors,
}

impl Read<'_> {
    pub fn poll(&mut self, process: &Process<'_>, index: usize) -> Option<SyscallResult> {
        let OpenFileDescription::Signalfd { mask, flags } = self.description.as_ref() else {
            unreachable!()
        };
        let mut copied = 0;
        while copied + SIZE <= self.buffers.length() {
            let thread = &process.threads[index].signals;
            let next = thread
                .borrow()
                .pending
                .first(mask.get())
                .map(|info| (info, true))
                .or_else(|| {
                    process
                        .signals
                        .borrow()
                        .pending
                        .first(mask.get())
                        .map(|info| (info, false))
                });
            let Some((info, local)) = next else { break };
            let result = self.buffers.scatter_at(process, copied, &mut encode(info));
            if !matches!(result, Ok(SIZE)) {
                return Some(if copied == 0 { Err(EFAULT) } else { Ok(copied) });
            }
            let selected = crate::process::signal::bit(info.number);
            if local {
                thread.borrow_mut().pending.take(selected);
            } else {
                process.signals.borrow_mut().pending.take(selected);
            }
            copied += SIZE;
        }
        if copied > 0 {
            Some(Ok(copied))
        } else if flags.get() & O_NONBLOCK != 0 {
            Some(Err(EAGAIN))
        } else {
            None
        }
    }
}

pub(in crate::syscall) fn ready(process: &Process<'_>, index: usize, mask: u64) -> bool {
    (process.threads[index].signals.borrow().pending.mask()
        | process.signals.borrow().pending.mask())
        & mask
        != 0
}

// REFERENCE: https://github.com/torvalds/linux/blob/master/include/uapi/linux/signalfd.h
fn encode(info: crate::process::signal::Info) -> [u8; SIZE] {
    let mut bytes = [0; SIZE];
    bytes[..4].copy_from_slice(&u32::from(info.number).to_le_bytes());
    bytes[8..12].copy_from_slice(&info.code.to_le_bytes());
    if info.number == 17 && info.code > 0 {
        bytes[12..16].copy_from_slice(&(info.sender as u32).to_le_bytes());
        bytes[16..20].copy_from_slice(&info.uid.to_le_bytes());
        bytes[40..44].copy_from_slice(&(info.value as u32).to_le_bytes());
    } else if info.code > 0 && matches!(info.number, 4 | 5 | 7 | 8 | 11) {
        bytes[72..80].copy_from_slice(&(info.address as u64).to_le_bytes());
    } else {
        bytes[12..16].copy_from_slice(&(info.sender as u32).to_le_bytes());
        bytes[16..20].copy_from_slice(&info.uid.to_le_bytes());
        bytes[44..48].copy_from_slice(&(info.value as u32).to_le_bytes());
        bytes[48..56].copy_from_slice(&info.value.to_le_bytes());
    }
    bytes
}
