// SPDX-License-Identifier: MPL-2.0

//! RTC 会话的时间查询与关闭处理。

use super::{Rtc, session::NOW};
use crate::{
    Error,
    device::{BUSY, INVALID_REQUEST, OpenRequest, SUCCESS, UNAVAILABLE},
};
use alloc::vec::Vec;
use runtime::{
    Channel, Handle, KernelError, Reply,
    ipc::{self, Message, ReceivedMessage, WaitItem, WaitMode},
};

pub fn run(rtc: &mut Rtc<'_>, control: &Handle<Channel>, device: u32) -> Result<(), Error> {
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
                    receive(rtc, request)?;
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

fn receive(rtc: &mut Rtc<'_>, mut message: ReceivedMessage) -> Result<(), Error> {
    let Some(reply) = message.take_reply() else {
        return Ok(());
    };
    if message.tag != NOW || !message.words().is_empty() || !message.handles().is_empty() {
        deliver(reply, INVALID_REQUEST, &[])?;
        return Ok(());
    }
    match rtc.now() {
        Ok(time) => {
            deliver(
                reply,
                SUCCESS,
                &[time.seconds as u64, time.nanoseconds.into()],
            )?;
        }
        Err(error) => {
            deliver(
                reply,
                if error == Error::Busy {
                    BUSY
                } else {
                    UNAVAILABLE
                },
                &[],
            )?;
        }
    }
    Ok(())
}

fn deliver(reply: Handle<Reply>, tag: u64, words: &[u64]) -> Result<bool, Error> {
    match reply.send(&mut Message::new(tag, words)?) {
        Ok(()) => Ok(true),
        Err(runtime::Error::Kernel(KernelError::InvalidState)) => Ok(false),
        Err(error) => Err(error.into()),
    }
}
