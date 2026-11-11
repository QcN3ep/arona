// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Entropy request queuing, completion delivery, and session reclamation.

use super::{MAX_BYTES, Rng, session::READ};
use crate::{
    Error,
    device::{INVALID_REQUEST, OpenRequest, SUCCESS, UNAVAILABLE},
};
use alloc::{collections::VecDeque, vec::Vec};
use runtime::{
    Channel, Event, FrameAllocation, Handle, KernelError, Reply, Rights,
    ipc::{self, Message, ReceivedMessage, WaitItem, WaitMode},
};

struct Connection {
    channel: Handle<Channel>,
    pending: VecDeque<Request>,
}

struct Request {
    output: Handle<FrameAllocation>,
    reply: Handle<Reply>,
    length: usize,
}

pub fn run(
    rng: &mut Rng<'_>,
    event: &Handle<Event>,
    control: &Handle<Channel>,
    device: u32,
) -> Result<(), Error> {
    let result = serve(rng, event, control, device);
    let stopped = rng.stop();
    result.and(stopped)
}

struct State {
    connections: Vec<Connection>,
    active: Option<Request>,
}

fn serve(
    rng: &mut Rng<'_>,
    event: &Handle<Event>,
    control: &Handle<Channel>,
    device: u32,
) -> Result<(), Error> {
    let mut state = State {
        connections: Vec::new(),
        active: None,
    };
    loop {
        state.complete(rng)?;
        let mut again = false;
        match control.receive(WaitMode::Poll) {
            Ok(request) => {
                again = true;
                if let Some(request) = OpenRequest::decode(request)? {
                    if request.device != device {
                        request.reject(Error::Unsupported)?;
                    } else if deliver(request.reply, SUCCESS, &[])? {
                        state.connections.push(Connection {
                            channel: request.channel,
                            pending: VecDeque::new(),
                        });
                    }
                }
            }
            Err(runtime::Error::Kernel(KernelError::WouldBlock)) => {}
            Err(runtime::Error::Kernel(KernelError::EndOfStream)) => return Ok(()),
            Err(error) => return Err(error.into()),
        }
        again |= state.receive()?;
        again |= state.submit(rng)?;
        if again {
            continue;
        }
        let mut items = Vec::with_capacity(state.connections.len() + 2);
        items.push(WaitItem::new(event, 1));
        items.push(WaitItem::new(control, ipc::READABLE | ipc::PEER_CLOSED));
        items.extend(state.connections.iter().map(|connection| {
            WaitItem::new(&connection.channel, ipc::READABLE | ipc::PEER_CLOSED)
        }));
        let ready = ipc::wait_many(&items, WaitMode::Forever)?;
        if ready.index == 0 {
            rng.acknowledge_interrupt()?;
        }
    }
}

impl State {
    fn complete(&mut self, rng: &mut Rng<'_>) -> Result<(), Error> {
        let mut bytes = [0; MAX_BYTES];
        if let Some(count) = rng.poll(&mut bytes)? {
            let mut request = self.active.take().ok_or(Error::DeviceFault)?;
            if request.output.write(0, &bytes[..count]).is_ok() {
                deliver(request.reply, SUCCESS, &[count as u64])?;
            } else {
                deliver(request.reply, UNAVAILABLE, &[])?;
            }
        }
        Ok(())
    }

    fn receive(&mut self) -> Result<bool, Error> {
        let mut again = false;
        let mut index = 0;
        while index < self.connections.len() {
            match self.connections[index].channel.receive(WaitMode::Poll) {
                Ok(request) => {
                    receive(&mut self.connections[index], request)?;
                    again = true;
                    index += 1;
                }
                Err(runtime::Error::Kernel(KernelError::WouldBlock)) => index += 1,
                Err(runtime::Error::Kernel(KernelError::EndOfStream)) => {
                    self.connections.remove(index);
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(again)
    }

    fn submit(&mut self, rng: &mut Rng<'_>) -> Result<bool, Error> {
        if self.active.is_some() {
            return Ok(false);
        }
        let Some(index) = self
            .connections
            .iter()
            .position(|connection| !connection.pending.is_empty())
        else {
            return Ok(false);
        };
        let request = self.connections[index].pending.pop_front().unwrap();
        rng.request(request.length)?;
        self.active = Some(request);
        let connection = self.connections.remove(index);
        self.connections.push(connection);
        Ok(true)
    }
}

fn receive(connection: &mut Connection, mut message: ReceivedMessage) -> Result<(), Error> {
    let Some(reply) = message.take_reply() else {
        return Ok(());
    };
    let result = decode(&mut message);
    match result {
        Ok((output, length)) => connection.pending.push_back(Request {
            output,
            reply,
            length,
        }),
        Err(_) => {
            deliver(reply, INVALID_REQUEST, &[])?;
        }
    }
    Ok(())
}

fn decode(message: &mut ReceivedMessage) -> Result<(Handle<FrameAllocation>, usize), Error> {
    if message.tag != READ || message.handles().len() != 1 {
        return Err(Error::InvalidRequest);
    }
    let [length] = *message.words() else {
        return Err(Error::InvalidRequest);
    };
    if length == 0 || length > MAX_BYTES as u64 {
        return Err(Error::InvalidRequest);
    }
    let output = message
        .take_handle(0)
        .unwrap()
        .downcast::<FrameAllocation>()
        .map_err(|_| Error::InvalidRequest)?;
    if !output.info()?.rights.contains(Rights::WRITE) || output.frames()? == 0 {
        return Err(Error::InvalidRequest);
    }
    Ok((output, length as usize))
}

fn deliver(reply: Handle<Reply>, tag: u64, words: &[u64]) -> Result<bool, Error> {
    match reply.send(&mut Message::new(tag, words)?) {
        Ok(()) => Ok(true),
        Err(runtime::Error::Kernel(KernelError::InvalidState)) => Ok(false),
        Err(error) => Err(error.into()),
    }
}
