// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Power session admission and action dispatch.

use super::{
    Error, Power,
    session::{CAPABILITIES, REBOOT, SHUTDOWN},
};
use crate::device::{BUSY, INVALID_REQUEST, OpenRequest, SUCCESS, UNAVAILABLE, UNSUPPORTED};
use alloc::vec::Vec;
use runtime::{
    Channel, Handle, KernelError, Reply,
    ipc::{self, Message, ReceivedMessage, WaitItem, WaitMode},
};

pub fn run(power: &mut Power<'_>, control: &Handle<Channel>, device: u32) -> Result<(), Error> {
    let mut connections: Vec<Handle<Channel>> = Vec::new();
    loop {
        let mut again = false;
        match control.receive(WaitMode::Poll) {
            Ok(request) => {
                again = true;
                if let Some(request) = OpenRequest::decode(request)? {
                    if request.device != device {
                        request.reject(Error::Unsupported)?;
                    } else if deliver(request.reply, SUCCESS, &[])? {
                        connections.push(request.channel);
                    }
                }
            }
            Err(runtime::Error::Kernel(KernelError::WouldBlock)) => {}
            Err(runtime::Error::Kernel(KernelError::EndOfStream)) => return Ok(()),
            Err(error) => return Err(error.into()),
        }
        let mut index = 0;
        while index < connections.len() {
            match connections[index].receive(WaitMode::Poll) {
                Ok(request) => {
                    receive(power, request)?;
                    again = true;
                    index += 1;
                }
                Err(runtime::Error::Kernel(KernelError::WouldBlock)) => index += 1,
                Err(runtime::Error::Kernel(KernelError::EndOfStream)) => {
                    connections.remove(index);
                }
                Err(error) => return Err(error.into()),
            }
        }
        if again {
            continue;
        }
        let mut items = Vec::with_capacity(connections.len() + 1);
        items.push(WaitItem::new(control, ipc::READABLE | ipc::PEER_CLOSED));
        items.extend(
            connections
                .iter()
                .map(|channel| WaitItem::new(channel, ipc::READABLE | ipc::PEER_CLOSED)),
        );
        ipc::wait_many(&items, WaitMode::Forever)?;
    }
}

fn receive(power: &mut Power<'_>, mut message: ReceivedMessage) -> Result<(), Error> {
    let Some(reply) = message.take_reply() else {
        return Ok(());
    };
    if !message.handles().is_empty() {
        deliver(reply, INVALID_REQUEST, &[])?;
        return Ok(());
    }
    match (message.tag, message.words()) {
        (CAPABILITIES, []) => {
            let capabilities = power.capabilities();
            deliver(
                reply,
                SUCCESS,
                &[capabilities.shutdown as u64, capabilities.reboot as u64],
            )?;
        }
        (SHUTDOWN, []) => action(reply, power.shutdown())?,
        (REBOOT, []) => action(reply, power.reboot())?,
        _ => {
            deliver(reply, INVALID_REQUEST, &[])?;
        }
    }
    Ok(())
}

fn action(reply: Handle<Reply>, result: Result<(), Error>) -> Result<(), Error> {
    match result {
        Ok(()) => deliver(reply, SUCCESS, &[])?,
        Err(error) => deliver(
            reply,
            match error {
                Error::Unsupported => UNSUPPORTED,
                Error::Busy => BUSY,
                Error::InvalidRequest | Error::InvalidConfiguration => INVALID_REQUEST,
                _ => UNAVAILABLE,
            },
            &[],
        )?,
    };
    Ok(())
}

fn deliver(reply: Handle<Reply>, tag: u64, words: &[u64]) -> Result<bool, Error> {
    match reply.send(&mut Message::new(tag, words)?) {
        Ok(()) => Ok(true),
        Err(runtime::Error::Kernel(KernelError::InvalidState)) => Ok(false),
        Err(error) => Err(error.into()),
    }
}
