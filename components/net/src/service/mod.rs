// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

mod operation;
mod session;

use crate::{Error, Readiness, protocol as p};
use alloc::vec::Vec;
use runtime::{
    Channel, Handle,
    ipc::{self, Message, WaitItem, WaitMode},
    process, thread,
};
use session::Session;

pub fn run(control: Handle<Channel>) -> Result<(), Error> {
    let mut startup = control.receive(WaitMode::Forever)?;
    let reply = startup.take_reply().ok_or(Error::InvalidRequest)?;
    if startup.tag != p::START || !startup.words().is_empty() || startup.handles().len() != 1 {
        return Err(Error::InvalidRequest);
    }
    let entropy =
        devmgr::rng::Session::from_channel(process::task(), p::take(&mut startup, p::CHANNEL)?)
            .map_err(|_| Error::InvalidRequest)?;
    let mut seed = [0; 32];
    entropy
        .fill(
            &mut seed,
            thread::now_ns()?
                .checked_add(10_000_000_000)
                .ok_or(Error::InvalidRequest)?,
        )
        .map_err(|_| Error::Unavailable)?;
    drop(entropy);
    let stack = lwip::Stack::initialize(seed).map_err(error)?;
    // TODO: Attach Ethernet frame sessions and configure external interfaces.
    reply.send(&mut Message::new(0, &[])?)?;
    let mut sessions: Vec<Option<Session>> = Vec::new();
    loop {
        match control.receive(WaitMode::Poll) {
            Ok(mut message) => {
                if message.tag == p::ATTACH
                    && message.words().is_empty()
                    && message.handles().len() == 1
                    && let Ok(channel) = p::take(&mut message, p::OWNED_CHANNEL)
                    && sessions.try_reserve(1).is_ok()
                {
                    sessions.push(Some(Session::new(channel)));
                }
            }
            Err(runtime::Error::Kernel(runtime::KernelError::WouldBlock)) => {}
            Err(runtime::Error::Kernel(runtime::KernelError::EndOfStream)) => return Ok(()),
            Err(error) => return Err(error.into()),
        }
        let mut queued = false;
        for index in 0..sessions.len() {
            let Some(mut session) = sessions[index].take() else {
                continue;
            };
            if session.poll()? {
                // Reserve the session slot before accept can consume a connection.
                let reserve = sessions.try_reserve(1).map_err(|_| Error::NoMemory);
                if let Some(child) = session.progress(&stack, reserve)? {
                    sessions.push(Some(child));
                }
                queued |= session.queued();
                sessions[index] = Some(session);
            }
        }
        sessions.retain(Option::is_some);
        if queued {
            continue;
        }
        let mut waits = Vec::new();
        waits
            .try_reserve(
                1 + sessions
                    .iter()
                    .flatten()
                    .map(Session::wait_count)
                    .sum::<usize>(),
            )
            .map_err(|_| Error::NoMemory)?;
        waits.push(WaitItem::new(&control, ipc::READABLE | ipc::PEER_CLOSED));
        for session in sessions.iter().flatten() {
            session.wait_items(&mut waits);
        }
        ipc::wait_many(&waits, WaitMode::Forever)?;
    }
}

fn error(error: lwip::Error) -> Error {
    match error {
        lwip::Error::WouldBlock => Error::WouldBlock,
        lwip::Error::InProgress => Error::InProgress,
        lwip::Error::AddressInUse => Error::AddressInUse,
        lwip::Error::ConnectionRefused => Error::ConnectionRefused,
        lwip::Error::ConnectionReset => Error::ConnectionReset,
        lwip::Error::NotConnected => Error::NotConnected,
        lwip::Error::NoMemory => Error::NoMemory,
        lwip::Error::InvalidArgument => Error::InvalidRequest,
        lwip::Error::Runtime(error) => error.into(),
        lwip::Error::Other(32) => Error::BrokenPipe,
        lwip::Error::Other(110) => Error::TimedOut,
        lwip::Error::Other(95 | 92) => Error::Unsupported,
        _ => Error::Io,
    }
}

fn respond(reply: &Handle<runtime::Reply>, tag: u64, words: &[u64]) -> Result<(), Error> {
    match reply.send(&mut Message::new(tag, words)?) {
        Ok(()) | Err(runtime::Error::Kernel(runtime::KernelError::InvalidState)) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub fn flags(state: Readiness) -> u64 {
    u64::from(state.readable)
        | (u64::from(state.writable) << 1)
        | (u64::from(state.error) << 2)
        | (u64::from(state.read_closed) << 3)
        | (u64::from(state.write_closed) << 4)
        | (u64::from(state.closed) << 5)
}
pub fn code(error: Error) -> u64 {
    match error {
        Error::WouldBlock => 1,
        Error::InProgress => 2,
        Error::Busy => 3,
        Error::Canceled => 4,
        Error::InvalidRequest => 5,
        Error::NoMemory => 6,
        Error::Unavailable => 7,
        Error::Closed => 8,
        Error::AddressInUse => 9,
        Error::ConnectionRefused => 10,
        Error::ConnectionReset => 11,
        Error::NotConnected => 12,
        Error::BrokenPipe => 13,
        Error::TimedOut => 14,
        Error::Unsupported => 15,
        Error::Io | Error::Runtime(_) => 16,
    }
}
