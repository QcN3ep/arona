// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Reply delivery and dispatch for device sessions without pending I/O.

use crate::{Error, device::OpenRequest, protocol::SUCCESS};
use alloc::vec::Vec;
use runtime::{
    Channel, Handle, KernelError, Reply,
    ipc::{self, Message, ReceivedMessage, WaitItem, WaitMode},
};

pub(crate) fn deliver(reply: &Handle<Reply>, message: &mut Message) -> Result<bool, Error> {
    match reply.send(message) {
        Ok(()) => Ok(true),
        Err(runtime::Error::Kernel(KernelError::InvalidState)) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn respond(reply: &Handle<Reply>, tag: u64, words: &[u64]) -> Result<bool, Error> {
    deliver(reply, &mut Message::new(tag, words)?)
}

pub(crate) fn serve(
    control: &Handle<Channel>,
    device: u32,
    mut receive: impl FnMut(ReceivedMessage) -> Result<(), Error>,
) -> Result<(), Error> {
    let mut connections: Vec<Handle<Channel>> = Vec::new();
    loop {
        let mut again = false;
        match control.receive(WaitMode::Poll) {
            Ok(request) => {
                again = true;
                if let Some(request) = OpenRequest::decode(request)? {
                    if request.device != device {
                        request.reject(Error::Unsupported)?;
                    } else if respond(&request.reply, SUCCESS, &[])? {
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
                    receive(request)?;
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
