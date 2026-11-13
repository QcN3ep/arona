// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Linux IP sockets over the native network service.

mod address;
mod message;
mod operation;
mod options;

use super::{
    Outcome,
    abi::*,
    file::{Buffer, O_CLOEXEC},
    wait,
};
use crate::{
    file::{
        Descriptor, O_NONBLOCK, OpenFileDescription,
        socket::{Connection, Socket},
    },
    process::Process,
};
use address::Output;
use alloc::{boxed::Box, rc::Rc};
use message::Receive;
pub(super) use operation::Pending;
use operation::{Action, Operation};

const DONTWAIT: usize = 0x40;
const NOSIGNAL: usize = 0x4000;
const MAX_STREAM: usize = 65536;

pub(super) fn dispatch<'a>(
    process: &mut Process<'a>,
    services: &crate::Services<'a>,
    number: usize,
    args: [usize; 6],
) -> Option<Outcome<'a>> {
    if number == SOCKET {
        return Some(outcome(create(services, args)));
    }
    if !matches!(
        number,
        BIND | LISTEN
            | ACCEPT
            | ACCEPT4
            | CONNECT
            | GETSOCKNAME
            | GETPEERNAME
            | SENDTO
            | RECVFROM
            | SETSOCKOPT
            | GETSOCKOPT
            | SHUTDOWN
            | SENDMSG
            | RECVMSG
    ) {
        return None;
    }
    Some(outcome((|| {
        let description = description(process, args[0])?;
        if matches!(number, GETSOCKOPT | SETSOCKOPT) {
            return options::dispatch(process, description, args, number == SETSOCKOPT);
        }
        let action = action(process, &description, number, args)?;
        let nonblocking = (matches!(number, SENDTO | RECVFROM) && args[3] & DONTWAIT != 0)
            || (matches!(number, SENDMSG | RECVMSG) && args[2] & DONTWAIT != 0);
        Ok(Pending::Operation(Box::new(Operation::new(
            description,
            action,
            nonblocking,
            process,
        )?)))
    })()))
}

fn action(
    process: &Process<'_>,
    description: &OpenFileDescription<'_>,
    number: usize,
    args: [usize; 6],
) -> Result<Action, Errno> {
    Ok(match number {
        BIND => {
            let endpoint = address::input(process, args[1], args[2])?;
            address::check_family(endpoint, Socket::state(description))?;
            if !endpoint.address.is_unspecified() && !endpoint.address.is_loopback() {
                return Err(EADDRNOTAVAIL);
            }
            Action::Bind(endpoint)
        }
        LISTEN => Action::Listen((args[1] as i32).clamp(0, 255) as u8),
        CONNECT => connect(process, description, args)?,
        ACCEPT | ACCEPT4 => {
            let flags = if number == ACCEPT4 { args[3] as u32 } else { 0 };
            if flags & !(O_NONBLOCK | O_CLOEXEC) != 0 {
                return Err(EINVAL);
            }
            Action::Accept {
                peer: peer_output(process, args[1], args[2])?,
                flags,
            }
        }
        GETSOCKNAME | GETPEERNAME => Action::Endpoint {
            peer: number == GETPEERNAME,
            output: Output::new(process, args[1], args[2])?,
        },
        SENDTO => {
            let peer = address::optional_input(process, args[4], args[5])?;
            message::send(
                process,
                Socket::state(description),
                Buffer::Slice {
                    address: args[1],
                    length: args[2],
                },
                peer,
                args[3],
            )?
        }
        RECVFROM => Action::Receive(Receive::new(
            process,
            Buffer::Slice {
                address: args[1],
                length: args[2],
            },
            peer_output(process, args[4], args[5])?,
            args[3],
            Socket::state(description).kind,
        )?),
        SENDMSG | RECVMSG => message::import(
            process,
            Socket::state(description),
            args[1],
            args[2],
            number == SENDMSG,
        )?,
        SHUTDOWN => Action::Shutdown(match args[1] as i32 {
            0 => net::Shutdown::Read,
            1 => net::Shutdown::Write,
            2 => net::Shutdown::Both,
            _ => return Err(EINVAL),
        }),
        _ => unreachable!(),
    })
}

fn connect(
    process: &Process<'_>,
    description: &OpenFileDescription<'_>,
    args: [usize; 6],
) -> Result<Action, Errno> {
    if Socket::state(description).kind == net::SocketKind::Tcp {
        match Socket::state(description).connection.get() {
            Connection::Connected => return Err(EISCONN),
            Connection::Initial | Connection::Connecting => {}
        }
    } else if (args[2] as i32) >= 2 && wait::read_bytes(process, args[1], 2)? == [0, 0] {
        return Ok(Action::Disconnect);
    }
    let endpoint = address::input(process, args[1], args[2])?;
    address::check_family(endpoint, Socket::state(description))?;
    address::route(endpoint)?;
    Ok(Action::Connect {
        endpoint,
        finishing: false,
    })
}

fn create<'a>(services: &crate::Services<'a>, args: [usize; 6]) -> Result<Pending<'a>, Errno> {
    let family = match args[0] as i32 {
        2 => net::AddressFamily::Ipv4,
        10 => net::AddressFamily::Ipv6,
        _ => return Err(EAFNOSUPPORT),
    };
    let flags = args[1] as u32;
    let kind = match (flags & !(O_NONBLOCK | O_CLOEXEC), args[2] as i32) {
        (1, 0 | 6) => net::SocketKind::Tcp,
        (2, 0 | 17) => net::SocketKind::Udp,
        _ => return Err(EPROTONOSUPPORT),
    };
    let native = services
        .net
        .as_ref()
        .ok_or(ENETUNREACH)?
        .start_socket_in(kind, family)
        .map_err(error)?;
    Ok(Pending::Create {
        native: Some(native),
        flags,
    })
}

pub(super) fn io<'a>(
    process: &Process<'a>,
    fd: usize,
    buffer: Buffer,
    offset: Option<usize>,
    write: bool,
) -> Outcome<'a> {
    (|| {
        let description = description(process, fd)?;
        if offset.is_some() {
            return Err(ESPIPE);
        }
        let action = if write {
            message::send(process, Socket::state(&description), buffer, None, 0)?
        } else {
            let receive = Receive::new(process, buffer, None, 0, Socket::state(&description).kind)?;
            // Unlike recv, a zero-length read leaves a pending datagram untouched.
            if receive.buffers.total_length() == 0 {
                return Ok(Outcome::Complete(Ok(0)));
            }
            Action::Receive(receive)
        };
        let operation = Operation::new(description, action, false, process)?;
        Ok(outcome(Ok(Pending::Operation(Box::new(operation)))))
    })()
    .unwrap_or_else(|error| Outcome::Complete(Err(error)))
}

fn check_flags(flags: usize, supported: usize) -> Result<(), Errno> {
    if flags & !supported == 0 {
        Ok(())
    } else {
        Err(EOPNOTSUPP)
    }
}

fn peer_output(
    process: &Process<'_>,
    address: usize,
    length: usize,
) -> Result<Option<Output>, Errno> {
    if address == 0 {
        Ok(None)
    } else {
        Output::new(process, address, length).map(Some)
    }
}

fn description<'a>(process: &Process<'a>, fd: usize) -> Result<Rc<OpenFileDescription<'a>>, Errno> {
    let descriptor = process.files.get(fd as i32 as usize).ok_or(EBADF)?;
    if !matches!(
        descriptor.description.as_ref(),
        OpenFileDescription::Socket { .. }
    ) {
        return Err(ENOTSOCK);
    }
    Ok(Rc::clone(&descriptor.description))
}

fn outcome(result: Result<Pending<'_>, Errno>) -> Outcome<'_> {
    match result {
        Ok(pending) => Outcome::Pending(super::Operation::Socket(pending)),
        Err(error) => Outcome::Complete(Err(error)),
    }
}

fn install<'a>(
    process: &mut Process<'a>,
    native: net::Socket<'a>,
    flags: u32,
    connected: bool,
    timeouts: [Option<u64>; 2],
) -> SyscallResult {
    let state = Socket::new(native, connected);
    state.receive_timeout.set(timeouts[0]);
    state.send_timeout.set(timeouts[1]);
    process
        .files
        .insert(
            Descriptor {
                description: Rc::new(OpenFileDescription::Socket {
                    state,
                    flags: core::cell::Cell::new(2 | (flags & O_NONBLOCK)),
                }),
                close_on_exec: flags & O_CLOEXEC != 0,
            },
            0,
        )
        .ok_or(EMFILE)
}

pub(super) fn error(error: net::Error) -> Errno {
    match error {
        net::Error::WouldBlock => EAGAIN,
        net::Error::InProgress => EINPROGRESS,
        net::Error::Busy => EALREADY,
        net::Error::Canceled => EINTR,
        net::Error::InvalidRequest => EINVAL,
        net::Error::NoMemory => ENOMEM,
        net::Error::AddressInUse => EADDRINUSE,
        net::Error::ConnectionRefused => ECONNREFUSED,
        net::Error::ConnectionReset => ECONNRESET,
        net::Error::NotConnected => ENOTCONN,
        net::Error::BrokenPipe | net::Error::Closed => EPIPE,
        net::Error::TimedOut => ETIMEDOUT,
        net::Error::Unsupported => EOPNOTSUPP,
        net::Error::Unavailable | net::Error::Io | net::Error::Runtime(_) => EIO,
    }
}
