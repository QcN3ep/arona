// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Pipe creation and blocking I/O.

use super::super::{
    Outcome,
    abi::{
        EAGAIN, EBADF, EBUSY, EFAULT, EINVAL, EMFILE, ENOMEM, EPERM, EPIPE, ESPIPE, SyscallResult,
    },
};
use super::{Buffer, O_CLOEXEC, Pending, buffer::IoVectors, fd, outcome};
use crate::{
    file::{
        Descriptor, O_NONBLOCK, OpenFileDescription,
        pipe::{CapacityError, PIPE_BUF, PipeEnd},
    },
    process::Process,
};
use alloc::{rc::Rc, vec::Vec};
use core::cell::Cell;

pub(in crate::syscall) enum PipeIo<'a> {
    Read {
        description: Rc<OpenFileDescription<'a>>,
        buffers: IoVectors,
    },
    Write {
        description: Rc<OpenFileDescription<'a>>,
        bytes: Vec<u8>,
        atomic: bool,
    },
}

pub(super) fn create(process: &mut Process<'_>, address: usize, flags: usize) -> SyscallResult {
    let flags = flags as u32;
    if flags & !(O_CLOEXEC | O_NONBLOCK) != 0 {
        return Err(EINVAL);
    }
    let (reader, writer) = PipeEnd::pair().map_err(|()| ENOMEM)?;
    let descriptor = |end: PipeEnd| Descriptor {
        close_on_exec: flags & O_CLOEXEC != 0,
        description: Rc::new(OpenFileDescription::Pipe {
            flags: Cell::new(u32::from(!end.read) | flags & O_NONBLOCK),
            end,
        }),
    };
    let first = process.files.insert(descriptor(reader), 0).ok_or(EMFILE)?;
    let Some(second) = process.files.insert(descriptor(writer), 0) else {
        process.files.close(first);
        return Err(EMFILE);
    };
    let mut bytes = [0; 8];
    bytes[..4].copy_from_slice(&(first as i32).to_le_bytes());
    bytes[4..].copy_from_slice(&(second as i32).to_le_bytes());
    if process.address_space.write(address, &bytes).is_err() {
        process.files.close(first);
        process.files.close(second);
        return Err(EFAULT);
    }
    Ok(0)
}

pub(super) fn capacity(description: &OpenFileDescription<'_>) -> SyscallResult {
    match description {
        OpenFileDescription::Pipe { end, .. } => Ok(end.capacity()),
        _ => Err(EBADF),
    }
}

pub(super) fn set_capacity(description: &OpenFileDescription<'_>, size: usize) -> SyscallResult {
    let OpenFileDescription::Pipe { end, .. } = description else {
        return Err(EBADF);
    };
    end.set_capacity(size).map_err(|error| match error {
        CapacityError::TooLarge => EPERM,
        CapacityError::Busy => EBUSY,
        CapacityError::NoMemory => ENOMEM,
    })
}

pub(super) fn start<'a>(
    process: &Process<'a>,
    raw: usize,
    buffer: Buffer,
    offset: Option<usize>,
    write: bool,
) -> Outcome<'a> {
    outcome((|| {
        let description = Rc::clone(&process.files.get(fd(raw)?).ok_or(EBADF)?.description);
        let OpenFileDescription::Pipe { end, .. } = description.as_ref() else {
            unreachable!()
        };
        if end.read == write {
            return Err(EBADF);
        }
        if offset.is_some() {
            return Err(ESPIPE);
        }
        let buffers = buffer.import(process)?;
        Ok(Pending::Pipe(if write {
            PipeIo::Write {
                atomic: buffers.total_length() <= PIPE_BUF,
                bytes: buffers.gather(process, buffers.length())?,
                description,
            }
        } else {
            PipeIo::Read {
                description,
                buffers,
            }
        }))
    })())
}

impl PipeIo<'_> {
    pub fn poll(&mut self, process: &Process<'_>) -> Option<SyscallResult> {
        let description = match self {
            Self::Read { description, .. } | Self::Write { description, .. } => {
                Rc::clone(description)
            }
        };
        let OpenFileDescription::Pipe { end, .. } = description.as_ref() else {
            unreachable!()
        };
        let nonblocking = description.flags() & O_NONBLOCK != 0;
        match self {
            Self::Read { buffers, .. } => {
                let length = buffers.length();
                if length == 0 {
                    return Some(Ok(0));
                }
                let mut bytes = end.snapshot(length);
                if bytes.is_empty() {
                    return if end.peer_closed() {
                        Some(Ok(0))
                    } else if nonblocking {
                        Some(Err(EAGAIN))
                    } else {
                        None
                    };
                }
                Some(buffers.scatter(process, &mut bytes).inspect(|&count| {
                    end.consume(count);
                }))
            }
            Self::Write { bytes, atomic, .. } => {
                if bytes.is_empty() {
                    return Some(Ok(0));
                }
                if end.peer_closed() {
                    return Some(Err(EPIPE));
                }
                let free = end.free();
                if free == 0 || *atomic && free < bytes.len() {
                    return if nonblocking { Some(Err(EAGAIN)) } else { None };
                }
                let count = free.min(bytes.len());
                end.write(&bytes[..count]);
                Some(Ok(count))
            }
        }
    }
}
