// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Unix socket pair creation and local stream or datagram I/O.

use super::{DONTWAIT, MAX_STREAM, NOSIGNAL, Output, check_flags, message, peer_output};
use crate::{
    file::{
        Descriptor, O_NONBLOCK, OpenFileDescription,
        socketpair::{MAX_DATAGRAM, SendError, SocketPair},
    },
    process::Process,
    syscall::{
        abi::*,
        file::{Buffer, O_CLOEXEC, buffer::IoVectors},
        wait::{self, Deadline},
    },
};
use alloc::{boxed::Box, rc::Rc, vec::Vec};
use core::cell::Cell;
use runtime::ipc::WaitItem;

pub(crate) struct Operation<'a> {
    description: Rc<OpenFileDescription<'a>>,
    action: LocalAction,
    nonblocking: bool,
    deadline: Deadline,
}

enum LocalAction {
    Send { bytes: Vec<u8> },
    Receive(Receive),
}

struct Receive {
    buffers: IoVectors,
    peer: Option<Output>,
    header: Option<usize>,
    flags: usize,
}

/// Handle `socketpair(2)` for an `AF_UNIX` stream or datagram pair.
pub(super) fn create(process: &mut Process<'_>, args: [usize; 6]) -> SyscallResult {
    if args[0] as i32 != 1 {
        return Err(EAFNOSUPPORT);
    }
    let raw = args[1] as u32;
    let allowed = O_NONBLOCK | O_CLOEXEC;
    if raw & !(allowed | 3) != 0 {
        return Err(EINVAL);
    }
    let kind = match raw & !allowed {
        1 => crate::file::socketpair::Kind::Stream,
        2 => crate::file::socketpair::Kind::Datagram,
        _ => return Err(EPROTONOSUPPORT),
    };
    if args[2] != 0 {
        return Err(EPROTONOSUPPORT);
    }
    let (first, second) = SocketPair::pair(kind).map_err(|()| ENOMEM)?;
    let descriptor = |state: SocketPair| Descriptor {
        node: None,
        description: Rc::new(OpenFileDescription::SocketPair {
            state,
            flags: Cell::new(2 | raw & O_NONBLOCK),
        }),
        close_on_exec: raw & O_CLOEXEC != 0,
    };
    let first = process.files.insert(descriptor(first), 0).ok_or(EMFILE)?;
    let Some(second) = process.files.insert(descriptor(second), 0) else {
        process.files.close(first);
        return Err(EMFILE);
    };
    let mut bytes = [0; 8];
    bytes[..4].copy_from_slice(&(first as i32).to_le_bytes());
    bytes[4..].copy_from_slice(&(second as i32).to_le_bytes());
    if process.address_space.write(args[3], &bytes).is_err() {
        process.files.close(first);
        process.files.close(second);
        return Err(EFAULT);
    }
    Ok(0)
}

pub(super) fn dispatch<'a>(
    process: &Process<'a>,
    description: Rc<OpenFileDescription<'a>>,
    number: usize,
    args: [usize; 6],
) -> Result<super::Pending<'a>, Errno> {
    match number {
        BIND | LISTEN | ACCEPT | ACCEPT4 => Err(EOPNOTSUPP),
        CONNECT => Err(EISCONN),
        GETSOCKNAME | GETPEERNAME => endpoint(process, &description, number == GETPEERNAME, args),
        SENDTO => sendto(process, description, args),
        RECVFROM => recvfrom(process, description, args),
        SENDMSG => sendmsg(process, description, args),
        RECVMSG => recvmsg(process, description, args),
        SHUTDOWN => shutdown(&description, args),
        SETSOCKOPT | GETSOCKOPT => options(process, &description, args, number == SETSOCKOPT),
        _ => Err(EOPNOTSUPP),
    }
}

pub(super) fn io<'a>(
    process: &Process<'a>,
    description: Rc<OpenFileDescription<'a>>,
    buffer: Buffer,
    offset: Option<usize>,
    write: bool,
) -> Result<super::Pending<'a>, Errno> {
    if offset.is_some() {
        return Err(ESPIPE);
    }
    let kind = SocketPair::state(&description).kind();
    let action = if write {
        send(process, &description, buffer, 0, 0)?
    } else {
        let receive = Receive::new(process, buffer, None, 0, kind)?;
        if receive.buffers.total_length() == 0 {
            return Ok(super::Pending::Done);
        }
        LocalAction::Receive(receive)
    };
    Ok(super::Pending::Local(Box::new(Operation::new(
        description,
        action,
        false,
        process,
    )?)))
}

fn sendto<'a>(
    process: &Process<'a>,
    description: Rc<OpenFileDescription<'a>>,
    args: [usize; 6],
) -> Result<super::Pending<'a>, Errno> {
    let action = send(
        process,
        &description,
        Buffer::Slice {
            address: args[1],
            length: args[2],
        },
        args[4],
        args[3],
    )?;
    Ok(super::Pending::Local(Box::new(Operation::new(
        description,
        action,
        args[3] & DONTWAIT != 0,
        process,
    )?)))
}

fn recvfrom<'a>(
    process: &Process<'a>,
    description: Rc<OpenFileDescription<'a>>,
    args: [usize; 6],
) -> Result<super::Pending<'a>, Errno> {
    let kind = SocketPair::state(&description).kind();
    let receive = Receive::new(
        process,
        Buffer::Slice {
            address: args[1],
            length: args[2],
        },
        peer_output(process, args[4], args[5])?,
        args[3],
        kind,
    )?;
    Ok(super::Pending::Local(Box::new(Operation::new(
        description,
        LocalAction::Receive(receive),
        args[3] & DONTWAIT != 0,
        process,
    )?)))
}

fn sendmsg<'a>(
    process: &Process<'a>,
    description: Rc<OpenFileDescription<'a>>,
    args: [usize; 6],
) -> Result<super::Pending<'a>, Errno> {
    let header = message::header(process, args[1])?;
    if header.name != 0 {
        return Err(EISCONN);
    }
    let action = send(process, &description, header.buffer, 0, args[2])?;
    Ok(super::Pending::Local(Box::new(Operation::new(
        description,
        action,
        args[2] & DONTWAIT != 0,
        process,
    )?)))
}

fn recvmsg<'a>(
    process: &Process<'a>,
    description: Rc<OpenFileDescription<'a>>,
    args: [usize; 6],
) -> Result<super::Pending<'a>, Errno> {
    let header = message::header(process, args[1])?;
    let peer = (header.name != 0)
        .then(|| Output::with_capacity(header.name, header.address + 8, header.length));
    let mut receive = Receive::new(
        process,
        header.buffer,
        peer,
        args[2],
        SocketPair::state(&description).kind(),
    )?;
    receive.header = Some(header.address);
    Ok(super::Pending::Local(Box::new(Operation::new(
        description,
        LocalAction::Receive(receive),
        args[2] & DONTWAIT != 0,
        process,
    )?)))
}

fn send(
    process: &Process<'_>,
    description: &OpenFileDescription<'_>,
    buffer: Buffer,
    peer: usize,
    flags: usize,
) -> Result<LocalAction, Errno> {
    check_flags(flags, DONTWAIT | NOSIGNAL)?;
    if peer != 0 {
        return Err(EISCONN);
    }
    let pair = SocketPair::state(description);
    let buffers = buffer.import(process)?;
    match pair.kind() {
        crate::file::socketpair::Kind::Datagram => {
            let total = buffers.total_length();
            if total > MAX_DATAGRAM {
                return Err(EMSGSIZE);
            }
            let bytes = buffers.gather(process, total)?;
            if bytes.len() != total {
                return Err(EFAULT);
            }
            Ok(LocalAction::Send { bytes })
        }
        crate::file::socketpair::Kind::Stream => Ok(LocalAction::Send {
            bytes: buffers.gather(process, MAX_STREAM)?,
        }),
    }
}

fn shutdown<'a>(
    description: &OpenFileDescription<'a>,
    args: [usize; 6],
) -> Result<super::Pending<'a>, Errno> {
    let direction = match args[1] as i32 {
        0 => net::Shutdown::Read,
        1 => net::Shutdown::Write,
        2 => net::Shutdown::Both,
        _ => return Err(EINVAL),
    };
    SocketPair::state(description).shutdown(direction);
    Ok(super::Pending::Done)
}

fn endpoint<'a>(
    process: &Process<'_>,
    _description: &OpenFileDescription<'a>,
    _peer: bool,
    args: [usize; 6],
) -> Result<super::Pending<'a>, Errno> {
    let output = Output::new(process, args[1], args[2])?;
    // An unnamed Unix socket has only its address family.
    output.bytes(process, &[1, 0])?;
    Ok(super::Pending::Done)
}

fn options<'a>(
    process: &Process<'a>,
    description: &OpenFileDescription<'a>,
    args: [usize; 6],
    write: bool,
) -> Result<super::Pending<'a>, Errno> {
    let pair = SocketPair::state(description);
    let level = args[1] as u32;
    let name = args[2] as u32;
    if level == 1 && matches!(name, 20 | 21) {
        return super::options::timeout(
            process,
            &pair.receive_timeout,
            &pair.send_timeout,
            args,
            write,
        );
    }
    if write {
        return Err(EOPNOTSUPP);
    }
    let value = match (level, name) {
        (1, 3) => match pair.kind() {
            crate::file::socketpair::Kind::Stream => 1,
            crate::file::socketpair::Kind::Datagram => 2,
        },
        (1, 4) => 0,
        (1, 7 | 8) => pair.capacity(),
        _ => return Err(ENOPROTOOPT),
    };
    Ok(super::Pending::Value(
        Output::new(process, args[3], args[4])?,
        value,
    ))
}

impl<'a> Operation<'a> {
    fn new(
        description: Rc<OpenFileDescription<'a>>,
        action: LocalAction,
        nonblocking: bool,
        process: &Process<'_>,
    ) -> Result<Self, Errno> {
        let pair = SocketPair::state(&description);
        let duration = if nonblocking || description.flags() & O_NONBLOCK != 0 {
            None
        } else {
            match action {
                LocalAction::Receive(_) => pair.receive_timeout.get(),
                LocalAction::Send { .. } => pair.send_timeout.get(),
            }
        };
        Ok(Self {
            description,
            action,
            nonblocking,
            deadline: Deadline::new(process.supervisor, duration)?,
        })
    }

    pub fn interrupt(&self) -> Option<(bool, SyscallResult)> {
        Some((self.deadline.deadline().is_none(), Err(EINTR)))
    }

    pub fn waits<'b>(&'b self, items: &mut Vec<WaitItem<'b>>) {
        self.deadline.waits(items);
    }

    pub(super) fn poll(&mut self, process: &Process<'_>) -> Option<SyscallResult> {
        let pair = SocketPair::state(&self.description);
        let nonblocking = self.nonblocking || self.description.flags() & O_NONBLOCK != 0;
        let attempt = match &mut self.action {
            LocalAction::Send { bytes } => send_attempt(pair, bytes, nonblocking),
            LocalAction::Receive(receive) => receive.attempt(process, pair),
        };
        if let Some(result) = attempt {
            return Some(result);
        }
        if nonblocking {
            return Some(Err(EAGAIN));
        }
        match self.deadline.expired() {
            Ok(true) => Some(Err(EAGAIN)),
            Ok(false) => None,
            Err(error) => Some(Err(error)),
        }
    }
}

fn send_attempt(
    pair: &SocketPair,
    bytes: &mut Vec<u8>,
    nonblocking: bool,
) -> Option<SyscallResult> {
    if bytes.is_empty() && pair.kind() == crate::file::socketpair::Kind::Stream {
        return Some(Ok(0));
    }
    if pair.write_broken() {
        return Some(Err(EPIPE));
    }
    match pair.kind() {
        crate::file::socketpair::Kind::Stream => {
            let free = pair.free();
            if free == 0 && nonblocking {
                return Some(Err(EAGAIN));
            }
            let count = pair.send_stream(bytes);
            (count != 0).then_some(Ok(count))
        }
        crate::file::socketpair::Kind::Datagram => {
            let length = bytes.len();
            if length > pair.capacity() {
                return Some(Err(EMSGSIZE));
            }
            match pair.send_message(bytes) {
                Ok(()) => Some(Ok(length)),
                Err(SendError::WouldBlock) if nonblocking => Some(Err(EAGAIN)),
                Err(SendError::WouldBlock) => None,
                Err(SendError::TooLarge) => Some(Err(EMSGSIZE)),
                Err(SendError::NoMemory) => Some(Err(ENOMEM)),
            }
        }
    }
}

impl Receive {
    fn new(
        process: &Process<'_>,
        buffer: Buffer,
        peer: Option<Output>,
        flags: usize,
        kind: crate::file::socketpair::Kind,
    ) -> Result<Self, Errno> {
        check_flags(
            flags,
            DONTWAIT
                | message::PEEK
                | if kind == crate::file::socketpair::Kind::Datagram {
                    message::TRUNC
                } else {
                    0
                },
        )?;
        Ok(Self {
            buffers: buffer.import(process)?,
            peer,
            header: None,
            flags,
        })
    }

    fn attempt(&self, process: &Process<'_>, pair: &SocketPair) -> Option<SyscallResult> {
        if pair.read_closed() {
            return Some(self.output(process, false).map(|()| 0));
        }
        if !pair.has_data() {
            if pair.peer_closed() {
                return Some(self.output(process, false).map(|()| 0));
            }
            return None;
        }
        let kind = pair.kind();
        let available = pair.available();
        let length = self.buffers.total_length();
        let count = length.min(available);
        let mut bytes = pair.snapshot(count);
        let copied = match self.buffers.scatter(process, &mut bytes) {
            Ok(copied) => copied,
            Err(error) => return Some(Err(error)),
        };
        if kind == crate::file::socketpair::Kind::Datagram && copied != count {
            return Some(Err(EFAULT));
        }
        let truncated = kind == crate::file::socketpair::Kind::Datagram && length < available;
        if let Err(error) = self.output(process, truncated) {
            return Some(Err(error));
        }
        if self.flags & message::PEEK == 0 {
            pair.consume(copied);
        }
        Some(Ok(
            if kind == crate::file::socketpair::Kind::Datagram && self.flags & message::TRUNC != 0 {
                available
            } else {
                copied
            },
        ))
    }

    fn output(&self, process: &Process<'_>, truncated: bool) -> Result<(), Errno> {
        if let Some(peer) = &self.peer {
            peer.bytes(process, &[])?;
        }
        if let Some(header) = self.header {
            if self.peer.is_none() {
                wait::write_bytes(process, header + 8, &0u32.to_le_bytes())?;
            }
            wait::write_bytes(process, header + 40, &0u64.to_le_bytes())?;
            wait::write_bytes(
                process,
                header + 48,
                &(if truncated { message::TRUNC as u32 } else { 0 }).to_le_bytes(),
            )?;
        }
        Ok(())
    }
}
