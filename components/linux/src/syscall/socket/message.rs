// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Socket payloads and explicit Linux msghdr translation.

use super::{DONTWAIT, MAX_STREAM, NOSIGNAL, Output, address, check_flags, operation::Action};
use crate::{
    file::socket::Socket,
    process::Process,
    syscall::{
        abi::*,
        file::{Buffer, buffer::IoVectors},
        wait,
    },
};

pub(super) const PEEK: usize = 2;
pub(super) const TRUNC: usize = 0x20;

pub(super) struct Receive {
    pub buffers: IoVectors,
    pub peer: Option<Output>,
    pub source: Option<net::Endpoint>,
    header: Option<usize>,
    flags: usize,
}

impl Receive {
    pub fn new(
        process: &Process<'_>,
        buffer: Buffer,
        peer: Option<Output>,
        flags: usize,
        kind: net::SocketKind,
    ) -> Result<Self, Errno> {
        check_flags(
            flags,
            DONTWAIT
                | PEEK
                | if kind == net::SocketKind::Udp {
                    TRUNC
                } else {
                    0
                },
        )?;
        Ok(Self {
            buffers: buffer.import(process)?,
            peer,
            source: None,
            header: None,
            flags,
        })
    }

    pub fn request(&self, kind: net::SocketKind) -> net::Request<'_> {
        if kind == net::SocketKind::Udp {
            net::Request::ReceiveDatagram
        } else if self.peer.is_some() && self.source.is_none() {
            net::Request::PeerEndpoint
        } else {
            net::Request::Receive(self.buffers.total_length().min(MAX_STREAM))
        }
    }

    pub fn buffered(
        &self,
        process: &Process<'_>,
        socket: &Socket<'_>,
    ) -> Result<Option<usize>, Errno> {
        let datagram = socket.kind == net::SocketKind::Udp;
        if !datagram && self.peer.is_some() && self.source.is_none() {
            return Ok(None);
        }
        let mut received = socket.received.borrow_mut();
        let Some(pending) = received.as_mut() else {
            if !datagram && socket.read_eof.get() {
                self.output(process, self.source, false)?;
                return Ok(Some(0));
            }
            return Ok(None);
        };
        let available = pending.data.len() - pending.offset;
        let length = self.buffers.total_length().min(available);
        let mut bytes = alloc::vec::Vec::new();
        bytes.try_reserve_exact(length).map_err(|_| ENOMEM)?;
        bytes.resize(length, 0);
        pending
            .data
            .read(pending.offset, &mut bytes)
            .map_err(super::error)?;
        let count = self.buffers.scatter(process, &mut bytes)?;
        if datagram && count != length {
            return Err(EFAULT);
        }
        let truncated = datagram && count < available;
        self.output(
            process,
            if datagram {
                pending.data.peer
            } else {
                self.source
            },
            truncated,
        )?;
        if self.flags & PEEK == 0 {
            pending.offset += count;
            if datagram || pending.offset == pending.data.len() {
                *received = None;
            }
        }
        Ok(Some(if datagram && self.flags & TRUNC != 0 {
            available
        } else {
            count
        }))
    }

    pub fn empty(&self, process: &Process<'_>) -> Result<(), Errno> {
        self.output(process, self.source, false)
    }

    fn output(
        &self,
        process: &Process<'_>,
        source: Option<net::Endpoint>,
        truncated: bool,
    ) -> Result<(), Errno> {
        if let Some(peer) = &self.peer {
            peer.endpoint(process, source.ok_or(EIO)?)?;
        }
        if let Some(header) = self.header {
            if self.peer.is_none() {
                wait::write_bytes(process, header + 8, &0u32.to_le_bytes())?;
            }
            wait::write_bytes(process, header + 40, &0u64.to_le_bytes())?;
            wait::write_bytes(
                process,
                header + 48,
                &(if truncated { TRUNC as u32 } else { 0 }).to_le_bytes(),
            )?;
        }
        Ok(())
    }
}

pub(super) fn send(
    process: &Process<'_>,
    socket: &Socket<'_>,
    buffer: Buffer,
    peer: Option<net::Endpoint>,
    flags: usize,
) -> Result<Action, Errno> {
    check_flags(flags, DONTWAIT | NOSIGNAL)?;
    let datagram = socket.kind == net::SocketKind::Udp;
    if datagram && let Some(peer) = peer {
        address::check_family(peer, socket)?;
        address::route(peer)?;
    }
    let buffers = buffer.import(process)?;
    let length = buffers.total_length();
    let maximum = if socket.family == net::AddressFamily::Ipv4 {
        65507
    } else {
        65487
    };
    if datagram && length > maximum {
        return Err(EMSGSIZE);
    }
    let bytes = buffers.gather(process, if datagram { maximum } else { MAX_STREAM })?;
    // A datagram is submitted only after every input byte has been copied.
    if datagram && bytes.len() != length {
        return Err(EFAULT);
    }
    Ok(Action::Send {
        bytes,
        peer: if datagram { peer } else { None },
    })
}

pub(super) fn import(
    process: &Process<'_>,
    socket: &Socket<'_>,
    header: usize,
    flags: usize,
    write: bool,
) -> Result<Action, Errno> {
    // REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/include/linux/socket.h?h=v6.12
    let bytes = wait::read_bytes(process, header, 56)?;
    let word = |offset| u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap()) as usize;
    let name = word(0);
    let length = i32::from_le_bytes(bytes[8..12].try_into().unwrap());
    if length < 0 {
        return Err(EINVAL);
    }
    if word(40) != 0 {
        return Err(EOPNOTSUPP);
    }
    let buffer = Buffer::Vector {
        address: word(16),
        count: word(24),
    };
    if write {
        let peer = address::optional_input(process, name, length as usize)?;
        send(process, socket, buffer, peer, flags)
    } else {
        let peer = (name != 0).then(|| Output::with_capacity(name, header + 8, length as usize));
        let mut receive = Receive::new(process, buffer, peer, flags, socket.kind)?;
        receive.header = Some(header);
        Ok(Action::Receive(receive))
    }
}
