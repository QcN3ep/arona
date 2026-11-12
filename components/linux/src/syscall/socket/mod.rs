// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Linux TCP sockets over the native network service.

mod address;
mod operation;

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
    ) {
        return None;
    }
    Some(outcome((|| {
        let description = description(process, args[0])?;
        if number == GETSOCKOPT && args[1] as u32 == 1 && args[2] as u32 == 3 {
            return Ok(Pending::Value(Output::new(process, args[3], args[4])?, 1));
        }
        let action = action(process, &description, number, args)?;
        let nonblocking = matches!(number, SENDTO | RECVFROM) && args[3] & DONTWAIT != 0;
        Ok(Pending::Operation(Box::new(Operation::new(
            description,
            action,
            nonblocking,
        ))))
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
            if endpoint.address != [0; 4] && endpoint.address[0] != 127 {
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
            check_flags(args[3], DONTWAIT | NOSIGNAL)?;
            if args[4] != 0 {
                address::input(process, args[4], args[5])?;
            }
            send_action(
                process,
                Buffer::Slice {
                    address: args[1],
                    length: args[2],
                },
            )?
        }
        RECVFROM => {
            check_flags(args[3], DONTWAIT)?;
            Action::Receive {
                buffers: Buffer::Slice {
                    address: args[1],
                    length: args[2],
                }
                .import(process)?,
                peer: peer_output(process, args[4], args[5])?,
                source: None,
            }
        }
        GETSOCKOPT => Action::GetOption {
            checking: false,
            name: if args[1] as u32 == 1 && args[2] as u32 == 4 {
                None
            } else {
                Some(address::option(args[1], args[2])?)
            },
            output: Output::new(process, args[3], args[4])?,
        },
        SETSOCKOPT => {
            let name = address::option(args[1], args[2])?;
            if (args[4] as u32) < 4 {
                return Err(EINVAL);
            }
            let bytes = wait::read_bytes(process, args[3], 4)?;
            Action::SetOption(name, i32::from_le_bytes(bytes.try_into().unwrap()) != 0)
        }
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
    match Socket::state(description).connection.get() {
        Connection::Connecting => return Err(EALREADY),
        Connection::Connected => return Err(EISCONN),
        Connection::Initial => {}
    }
    let endpoint = address::input(process, args[1], args[2])?;
    // TODO: Route non-loopback destinations after external interfaces are attached.
    if endpoint.address[0] != 127 {
        return Err(ENETUNREACH);
    }
    Ok(Action::Connect {
        endpoint,
        finishing: false,
    })
}

fn create<'a>(services: &crate::Services<'a>, args: [usize; 6]) -> Result<Pending<'a>, Errno> {
    if args[0] as i32 != 2 {
        return Err(EAFNOSUPPORT);
    }
    let flags = args[1] as u32;
    // TODO: Add datagram sockets and message I/O.
    if flags & !(O_NONBLOCK | O_CLOEXEC) != 1 || !matches!(args[2] as i32, 0 | 6) {
        return Err(EPROTONOSUPPORT);
    }
    let native = services
        .net
        .as_ref()
        .ok_or(ENETUNREACH)?
        .start_socket(net::SocketKind::Tcp)
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
    outcome((|| {
        let description = description(process, fd)?;
        if offset.is_some() {
            return Err(ESPIPE);
        }
        let action = if write {
            send_action(process, buffer)?
        } else {
            Action::Receive {
                buffers: buffer.import(process)?,
                peer: None,
                source: None,
            }
        };
        Ok(Pending::Operation(Box::new(Operation::new(
            description,
            action,
            false,
        ))))
    })())
}

fn send_action(process: &Process<'_>, buffer: Buffer) -> Result<Action, Errno> {
    let buffers = buffer.import(process)?;
    Ok(Action::Send(buffers.gather(process, MAX_STREAM)?))
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
) -> SyscallResult {
    process
        .files
        .insert(
            Descriptor {
                description: Rc::new(OpenFileDescription::Socket {
                    state: Socket::new(native, connected),
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
