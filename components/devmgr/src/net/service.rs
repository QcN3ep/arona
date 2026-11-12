// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Exclusive frame session dispatch, bounded receive buffering, and DMA completions.

use super::{
    Info, MAX_FRAME, MIN_FRAME, Net,
    session::{INFO, RECEIVE, TRANSMIT, WATCH},
};
use crate::{
    Error,
    device::{BUSY, INVALID_REQUEST, OpenRequest, SUCCESS, UNAVAILABLE},
};
use alloc::{collections::VecDeque, vec::Vec};
use runtime::{
    Channel, Event, FrameAllocation, Handle, KernelError, Reply, Rights,
    ipc::{self, Message, ReceivedMessage, WaitItem, WaitMode},
};

const RECEIVE_CAPACITY: usize = 32;

struct Connection {
    channel: Handle<Channel>,
    receive: Option<(Handle<FrameAllocation>, Handle<Reply>)>,
    watch: Option<(u64, Handle<Reply>)>,
}

struct State {
    connection: Option<Connection>,
    frames: VecDeque<Vec<u8>>,
    transmit: Vec<Option<Handle<Reply>>>,
}

/// Serve one exclusive session at a time. Control closure stops and resets the device.
/// RX overflow drops new frames. Pending DMA survives session closure until completion.
pub fn run(
    net: &mut Net<'_>,
    event: &Handle<Event>,
    control: &Handle<Channel>,
    device: u32,
) -> Result<(), Error> {
    let result = serve(net, event, control, device);
    let stopped = net.stop();
    result.and(stopped)
}

fn serve(
    net: &mut Net<'_>,
    event: &Handle<Event>,
    control: &Handle<Channel>,
    device: u32,
) -> Result<(), Error> {
    let mut frames = VecDeque::new();
    frames
        .try_reserve_exact(RECEIVE_CAPACITY)
        .map_err(|_| runtime::Error::Kernel(KernelError::NoMemory))?;
    let mut state = State {
        connection: None,
        frames,
        transmit: (0..net.transmit_capacity()).map(|_| None).collect(),
    };
    loop {
        // Handle configuration IRQs even while clients continuously submit requests.
        match ipc::wait_many(&[WaitItem::new(event, 1)], WaitMode::Poll) {
            Ok(_) => net.acknowledge_interrupt()?,
            Err(runtime::Error::Kernel(KernelError::WouldBlock)) => {}
            Err(error) => return Err(error.into()),
        }
        state.complete(net)?;
        let mut again = false;
        match control.receive(WaitMode::Poll) {
            Ok(message) => {
                again = true;
                if let Some(request) = OpenRequest::decode(message)? {
                    if request.device != device {
                        request.reject(Error::Unsupported)?;
                    } else if state.connection.is_some() {
                        request.reject(Error::Busy)?;
                    } else if deliver(request.reply, SUCCESS, &[])? {
                        state.connection = Some(Connection {
                            channel: request.channel,
                            receive: None,
                            watch: None,
                        });
                    }
                }
            }
            Err(runtime::Error::Kernel(KernelError::WouldBlock)) => {}
            Err(runtime::Error::Kernel(KernelError::EndOfStream)) => return Ok(()),
            Err(error) => return Err(error.into()),
        }
        if let Some(connection) = &state.connection {
            match connection.channel.receive(WaitMode::Poll) {
                Ok(message) => {
                    state.request(net, message)?;
                    again = true;
                }
                Err(runtime::Error::Kernel(KernelError::WouldBlock)) => {}
                Err(runtime::Error::Kernel(KernelError::EndOfStream)) => {
                    state.connection = None;
                    state.frames.clear();
                    again = true;
                }
                Err(error) => return Err(error.into()),
            }
        }
        state.deliver(net.info()?)?;
        if again {
            continue;
        }
        let mut items = alloc::vec![
            WaitItem::new(event, 1),
            WaitItem::new(control, ipc::READABLE | ipc::PEER_CLOSED)
        ];
        if let Some(connection) = &state.connection {
            items.push(WaitItem::new(
                &connection.channel,
                ipc::READABLE | ipc::PEER_CLOSED,
            ));
        }
        if ipc::wait_many(&items, WaitMode::Forever)?.index == 0 {
            net.acknowledge_interrupt()?;
        }
    }
}

impl State {
    fn complete(&mut self, net: &mut Net<'_>) -> Result<(), Error> {
        while let Some(slot) = net.poll_transmit()? {
            let reply = self
                .transmit
                .get_mut(usize::from(slot))
                .and_then(Option::take)
                .ok_or(Error::DeviceFault)?;
            deliver(reply, SUCCESS, &[])?;
        }
        let mut bytes = [0; MAX_FRAME];
        // Bound one dispatch turn so continuous RX cannot starve control or session closure.
        for _ in 0..RECEIVE_CAPACITY {
            let Some(length) = net.receive(&mut bytes)? else {
                break;
            };
            if self.connection.is_some() && self.frames.len() < RECEIVE_CAPACITY {
                let mut frame = Vec::new();
                if frame.try_reserve_exact(length).is_ok() {
                    frame.extend_from_slice(&bytes[..length]);
                    self.frames.push_back(frame);
                }
            }
        }
        Ok(())
    }

    fn deliver(&mut self, info: Info) -> Result<(), Error> {
        let Some(connection) = &mut self.connection else {
            return Ok(());
        };
        if connection
            .watch
            .as_ref()
            .is_some_and(|(sequence, _)| *sequence != info.sequence)
        {
            let (_, reply) = connection.watch.take().unwrap();
            deliver_info(reply, info)?;
        }
        if !self.frames.is_empty()
            && let Some((mut output, reply)) = connection.receive.take()
        {
            let frame = self.frames.front().unwrap();
            if output.write(0, frame).is_err() {
                deliver(reply, UNAVAILABLE, &[])?;
            } else if deliver(reply, SUCCESS, &[frame.len() as u64])? {
                self.frames.pop_front();
            }
        }
        Ok(())
    }

    fn request(&mut self, net: &mut Net<'_>, mut message: ReceivedMessage) -> Result<(), Error> {
        let Some(reply) = message.take_reply() else {
            return Ok(());
        };
        let connection = self.connection.as_mut().unwrap();
        match (message.tag, message.words(), message.handles().len()) {
            (INFO, [], 0) => {
                deliver_info(reply, net.info()?)?;
            }
            (WATCH, [sequence], 0) => {
                if connection.watch.is_some() {
                    deliver(reply, BUSY, &[])?;
                } else {
                    connection.watch = Some((*sequence, reply));
                }
            }
            (RECEIVE, [], 1) => {
                if let Ok(buffer) = buffer(&mut message, Rights::WRITE) {
                    if connection.receive.is_some() {
                        deliver(reply, BUSY, &[])?;
                    } else {
                        connection.receive = Some((buffer, reply));
                    }
                } else {
                    deliver(reply, INVALID_REQUEST, &[])?;
                }
            }
            (TRANSMIT, [length], 1)
                if (MIN_FRAME as u64..=net.info()?.max_frame() as u64).contains(length) =>
            {
                let length = *length as usize;
                let mut frame = [0; MAX_FRAME];
                let result = buffer(&mut message, Rights::READ).and_then(|buffer| {
                    buffer
                        .read(0, &mut frame[..length])
                        .map_err(|_| Error::InvalidRequest)?;
                    net.transmit(&frame[..length])
                });
                match result {
                    Ok(slot) => self.transmit[usize::from(slot)] = Some(reply),
                    Err(Error::Busy) => {
                        deliver(reply, BUSY, &[])?;
                    }
                    Err(Error::InvalidRequest) => {
                        deliver(reply, INVALID_REQUEST, &[])?;
                    }
                    Err(Error::Unavailable) => {
                        deliver(reply, UNAVAILABLE, &[])?;
                    }
                    Err(error) => return Err(error),
                }
            }
            _ => {
                deliver(reply, INVALID_REQUEST, &[])?;
            }
        }
        Ok(())
    }
}

fn buffer(message: &mut ReceivedMessage, rights: Rights) -> Result<Handle<FrameAllocation>, Error> {
    let buffer = message
        .take_handle(0)
        .unwrap()
        .downcast::<FrameAllocation>()
        .map_err(|_| Error::InvalidRequest)?;
    if !buffer.info()?.rights.contains(rights) || buffer.frames()? == 0 {
        return Err(Error::InvalidRequest);
    }
    Ok(buffer)
}

fn deliver_info(reply: Handle<Reply>, info: Info) -> Result<bool, Error> {
    let mut mac = [0; 8];
    mac[..6].copy_from_slice(&info.mac);
    deliver(
        reply,
        SUCCESS,
        &[
            u64::from_le_bytes(mac),
            u64::from(info.mtu),
            u64::from(info.link_up),
            info.sequence,
        ],
    )
}

fn deliver(reply: Handle<Reply>, tag: u64, words: &[u64]) -> Result<bool, Error> {
    match reply.send(&mut Message::new(tag, words)?) {
        Ok(()) => Ok(true),
        Err(runtime::Error::Kernel(KernelError::InvalidState)) => Ok(false),
        Err(error) => Err(error.into()),
    }
}
