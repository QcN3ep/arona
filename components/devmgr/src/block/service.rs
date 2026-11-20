// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Block device session dispatch, queue backpressure, and completion delivery.

use crate::protocol::SUCCESS;
use alloc::vec::Vec;
use runtime::{
    Channel, Event, FrameAllocation, Handle, KernelError, Reply,
    ipc::{self, Message, ReceivedMessage, WaitItem, WaitMode},
};

use super::{Block, CancelResult, session};
use crate::{Error, device::OpenRequest};

struct Connection {
    channel: Handle<Channel>,
    slot: u32,
    origin: u64,
    length: u64,
    acknowledgements: Vec<(u32, Handle<Reply>)>,
    active: Vec<u32>,
}

enum Response {
    Reply(Vec<u64>),
    Pending(u64),
    Delivered,
}

pub fn run(
    block: &mut Block<'_>,
    event: &Handle<Event>,
    control: &Handle<Channel>,
    device: u32,
) -> Result<(), Error> {
    let result = serve(block, event, control, device);
    let stopped = block.stop();
    result.and(stopped)
}

fn serve(
    block: &mut Block<'_>,
    event: &Handle<Event>,
    control: &Handle<Channel>,
    device: u32,
) -> Result<(), Error> {
    let mut connections: Vec<Connection> = Vec::new();
    let mut retired: Vec<u64> = Vec::new();
    let mut next_slot = 0u32;
    loop {
        block.poll()?;
        for client in &mut connections {
            complete(block, client)?;
        }
        cleanup(block, &mut retired);
        let mut again = false;
        loop {
            let request = match control.receive(WaitMode::Poll) {
                Ok(request) => request,
                Err(runtime::Error::Kernel(KernelError::WouldBlock)) => break,
                Err(runtime::Error::Kernel(KernelError::EndOfStream | KernelError::PeerClosed)) => {
                    return Ok(());
                }
                Err(error) => return Err(error.into()),
            };
            again = true;
            let Some(request) = OpenRequest::decode(request)? else {
                continue;
            };
            if request.device != device {
                request.reject(Error::Unsupported)?;
                continue;
            }
            let (origin, length) = if request.partition == 0 {
                (0, block.info().capacity)
            } else {
                match block
                    .partitions()
                    .get(request.partition as usize - 1)
                    .copied()
                    .flatten()
                {
                    Some(partition) if request.partition <= 4 => {
                        (partition.start, partition.length)
                    }
                    _ => {
                        let error = if request.partition <= 4 {
                            Error::NotFound
                        } else {
                            Error::InvalidRequest
                        };
                        request.reject(error)?;
                        continue;
                    }
                }
            };
            if next_slot == u32::MAX {
                request.reject(Error::Busy)?;
                continue;
            }
            if deliver(&request.reply, &mut Message::new(SUCCESS, &[])?)? {
                connections.push(Connection {
                    channel: request.channel,
                    slot: next_slot,
                    origin,
                    length,
                    acknowledgements: Vec::new(),
                    active: Vec::new(),
                });
                next_slot += 1;
            }
        }
        let mut index = 0;
        while index < connections.len() {
            let mut closed = false;
            for _ in 0..block.info().max_requests {
                match connections[index].channel.receive(WaitMode::Poll) {
                    Ok(request) => {
                        handle(block, &mut connections[index], request)?;
                        again = true;
                    }
                    Err(runtime::Error::Kernel(KernelError::WouldBlock)) => break,
                    Err(runtime::Error::Kernel(
                        KernelError::EndOfStream | KernelError::PeerClosed,
                    )) => {
                        closed = true;
                        break;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            if closed {
                retire(block, connections.swap_remove(index), &mut retired);
                continue;
            }
            index += 1;
        }
        block.submit()?;
        if again {
            continue;
        }
        let mut items = Vec::with_capacity(connections.len() + 2);
        items.push(WaitItem::new(event, 1));
        items.push(WaitItem::new(control, ipc::READABLE | ipc::PEER_CLOSED));
        items.extend(
            connections
                .iter()
                .map(|client| WaitItem::new(&client.channel, ipc::READABLE | ipc::PEER_CLOSED)),
        );
        let ready = ipc::wait_many(&items, WaitMode::Forever)?;
        if ready.index == 0 {
            block.acknowledge_interrupt()?;
        }
    }
}

fn handle(
    block: &mut Block<'_>,
    connection: &mut Connection,
    mut request: ReceivedMessage,
) -> Result<(), Error> {
    let Some(reply) = request.take_reply() else {
        return Ok(());
    };
    let tag = request.tag;
    let id = match request.words() {
        [id, ..] if *id <= u64::from(u32::MAX) => Some(*id as u32),
        [] => None,
        _ => {
            reject(&reply, Error::InvalidRequest)?;
            return Ok(());
        }
    };
    let result = dispatch(block, connection, &mut request, &reply);
    match result {
        Ok(Response::Reply(words)) => {
            if matches!(tag, session::READ | session::WRITE | session::FLUSH)
                && let Some(id) = id
            {
                connection.active.push(id);
            }
            deliver(&reply, &mut Message::new(SUCCESS, &words)?)?;
        }
        Ok(Response::Pending(id)) => {
            if connection.acknowledgements.len() >= block.info().max_requests {
                reject(&reply, Error::Busy)?;
            } else {
                connection.acknowledgements.push((id as u32, reply));
            }
        }
        Ok(Response::Delivered) => {
            if tag == session::ACK
                && let Some(id) = id
            {
                connection.active.retain(|active| *active != id);
            }
        }
        Err(error) => reject(&reply, error)?,
    }
    Ok(())
}

fn dispatch(
    block: &mut Block<'_>,
    connection: &Connection,
    request: &mut ReceivedMessage,
    reply: &Handle<Reply>,
) -> Result<Response, Error> {
    if matches!(request.tag, session::READ | session::WRITE) {
        let [id, offset, buffer_offset, length] = *request.words() else {
            return Err(Error::InvalidRequest);
        };
        if request.handles().len() != 1 || id > u64::from(u32::MAX) {
            return Err(Error::InvalidRequest);
        }
        let length = usize::try_from(length).map_err(|_| Error::InvalidRequest)?;
        let end = offset
            .checked_add(length as u64)
            .ok_or(Error::InvalidRequest)?;
        if end > connection.length {
            return Err(Error::InvalidRequest);
        }
        let offset = connection
            .origin
            .checked_add(offset)
            .ok_or(Error::InvalidRequest)?;
        let frames = request
            .take_handle(0)
            .unwrap()
            .downcast::<FrameAllocation>()
            .map_err(|_| Error::InvalidRequest)?;
        let buffer_offset = usize::try_from(buffer_offset).map_err(|_| Error::InvalidRequest)?;
        let id = internal(connection.slot, id as u32)?;
        if request.tag == session::READ {
            block.read(id, offset, frames, buffer_offset, length)?;
        } else {
            block.write(id, offset, &frames, buffer_offset, length)?;
        }
        return Ok(Response::Reply(Vec::new()));
    }
    if !request.handles().is_empty() {
        return Err(Error::InvalidRequest);
    }
    match (request.tag, request.words()) {
        (session::INFO, []) => {
            let info = block.info();
            Ok(Response::Reply(alloc::vec![
                connection.length,
                connection.origin,
                info.block_size.into(),
                u64::from(info.read_only) | u64::from(info.flush) << 1,
                info.max_transfer as u64,
                info.max_requests as u64
            ]))
        }
        (session::FLUSH, [id]) => {
            block.flush(internal(connection.slot, *id as u32)?)?;
            Ok(Response::Reply(Vec::new()))
        }
        (session::CANCEL, [id]) => Ok(Response::Reply(alloc::vec![match block
            .cancel(internal(connection.slot, *id as u32)?,)?
        {
            CancelResult::Cancelled => 0,
            CancelResult::Submitted => 1,
            CancelResult::Completed => 2,
        }])),
        (session::ACK, [id, wait @ 0..=1]) => {
            let id = *id as u32;
            if connection
                .acknowledgements
                .iter()
                .any(|(previous, _)| *previous == id)
            {
                return Err(Error::Busy);
            }
            let internal = internal(connection.slot, id)?;
            if let Some(completion) = block.completion(internal)? {
                let mut response = Message::new(
                    SUCCESS,
                    &[completion.status as u64, completion.bytes as u64],
                )?;
                if deliver(reply, &mut response)? {
                    block.acknowledge(internal)?;
                }
                Ok(Response::Delivered)
            } else if *wait == 1 {
                Ok(Response::Pending(u64::from(id)))
            } else {
                Err(Error::Busy)
            }
        }
        _ => Err(Error::InvalidRequest),
    }
}

fn complete(block: &mut Block<'_>, connection: &mut Connection) -> Result<(), Error> {
    let mut index = 0;
    while index < connection.acknowledgements.len() {
        let (id, reply) = &connection.acknowledgements[index];
        let internal = internal(connection.slot, *id)?;
        if let Some(completion) = block.completion(internal)? {
            if deliver(
                reply,
                &mut Message::new(
                    SUCCESS,
                    &[completion.status as u64, completion.bytes as u64],
                )?,
            )? {
                block.acknowledge(internal)?;
                connection.active.retain(|active| active != id);
                connection.acknowledgements.swap_remove(index);
            } else {
                index += 1;
            }
        } else {
            index += 1;
        }
    }
    Ok(())
}

fn retire(block: &mut Block<'_>, connection: Connection, retired: &mut Vec<u64>) {
    for id in connection.active {
        if let Ok(id) = internal(connection.slot, id) {
            let _ = block.cancel(id);
            retired.push(id);
        }
    }
    drop(connection.acknowledgements);
}

fn cleanup(block: &mut Block<'_>, retired: &mut Vec<u64>) {
    retired.retain_mut(|id| match block.completion(*id) {
        Ok(Some(_)) => {
            let _ = block.acknowledge(*id);
            false
        }
        Ok(None) => {
            let _ = block.cancel(*id);
            true
        }
        Err(Error::NotFound) => false,
        Err(_) => true,
    });
}

fn internal(slot: u32, id: u32) -> Result<u64, Error> {
    let prefix = u64::from(slot)
        .checked_add(1)
        .ok_or(Error::InvalidRequest)?;
    Ok(prefix << 32 | u64::from(id))
}

fn reject(reply: &Handle<Reply>, error: Error) -> Result<(), Error> {
    deliver(
        reply,
        &mut Message::new(crate::protocol::error_status(error), &[])?,
    )?;
    Ok(())
}

fn deliver(reply: &Handle<Reply>, message: &mut Message) -> Result<bool, Error> {
    match reply.send(message) {
        Ok(()) => Ok(true),
        Err(runtime::Error::Kernel(KernelError::InvalidState)) => Ok(false),
        Err(error) => Err(error.into()),
    }
}
