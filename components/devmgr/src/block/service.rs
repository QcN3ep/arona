// SPDX-License-Identifier: MPL-2.0

//! 块设备会话分发、队列背压与完成交付。

use alloc::vec::Vec;
use runtime::{
    Channel, Event, FrameAllocation, Handle, KernelError, Reply,
    ipc::{self, Message, ReceivedMessage, WaitItem, WaitMode},
};

use super::{Block, CancelResult, session};
use crate::{
    Error,
    device::{OpenRequest, SUCCESS},
};

struct Connection {
    channel: Handle<Channel>,
    acknowledgements: Vec<(u64, Handle<Reply>)>,
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
    let mut connection: Option<Connection> = None;
    loop {
        block.poll()?;
        if let Some(client) = &mut connection {
            complete(block, client)?;
        } else {
            block.discard();
        }
        let mut again = false;
        match control.receive(WaitMode::Poll) {
            Ok(request) => {
                again = true;
                if let Some(request) = OpenRequest::decode(request)? {
                    if request.device != device {
                        request.reject(Error::Unsupported)?;
                    } else if connection.is_some() || !block.idle() {
                        request.reject(Error::Busy)?;
                    } else if deliver(&request.reply, &mut Message::new(SUCCESS, &[])?)? {
                        connection = Some(Connection {
                            channel: request.channel,
                            acknowledgements: Vec::new(),
                        });
                    }
                }
            }
            Err(runtime::Error::Kernel(KernelError::WouldBlock)) => {}
            Err(runtime::Error::Kernel(KernelError::EndOfStream)) => return Ok(()),
            Err(error) => return Err(error.into()),
        }
        if let Some(client) = &mut connection {
            for _ in 0..block.info().max_requests {
                match client.channel.receive(WaitMode::Poll) {
                    Ok(request) => {
                        handle(block, client, request)?;
                        again = true;
                    }
                    Err(runtime::Error::Kernel(KernelError::WouldBlock)) => break,
                    Err(runtime::Error::Kernel(KernelError::EndOfStream)) => {
                        connection = None;
                        block.discard();
                        break;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }
        block.submit()?;
        if again {
            continue;
        }
        let mut items = [
            WaitItem::new(event, 1),
            WaitItem::new(control, ipc::READABLE | ipc::PEER_CLOSED),
            WaitItem::new(event, 1),
        ];
        let count = if let Some(client) = &connection {
            items[2] = WaitItem::new(&client.channel, ipc::READABLE | ipc::PEER_CLOSED);
            3
        } else {
            2
        };
        let ready = ipc::wait_many(&items[..count], WaitMode::Forever)?;
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
    let result = dispatch(block, connection, &mut request, &reply);
    match result {
        Ok(Response::Reply(words)) => {
            deliver(&reply, &mut Message::new(SUCCESS, &words)?)?;
        }
        Ok(Response::Pending(id)) => {
            if connection.acknowledgements.len() >= block.info().max_requests {
                reject(&reply, Error::Busy)?;
            } else {
                connection.acknowledgements.push((id, reply));
            }
        }
        Ok(Response::Delivered) => {}
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
        if request.handles().len() != 1 {
            return Err(Error::InvalidRequest);
        }
        let frames = request
            .take_handle(0)
            .unwrap()
            .downcast::<FrameAllocation>()
            .map_err(|_| Error::InvalidRequest)?;
        let buffer_offset = usize::try_from(buffer_offset).map_err(|_| Error::InvalidRequest)?;
        let length = usize::try_from(length).map_err(|_| Error::InvalidRequest)?;
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
                info.capacity,
                info.block_size.into(),
                u64::from(info.read_only) | u64::from(info.flush) << 1,
                info.max_transfer as u64,
                info.max_requests as u64
            ]))
        }
        (session::FLUSH, [id]) => {
            block.flush(*id)?;
            Ok(Response::Reply(Vec::new()))
        }
        (session::CANCEL, [id]) => Ok(Response::Reply(alloc::vec![match block.cancel(*id)? {
            CancelResult::Cancelled => 0,
            CancelResult::Submitted => 1,
            CancelResult::Completed => 2,
        }])),
        (session::ACK, [id, wait @ 0..=1]) => {
            if connection
                .acknowledgements
                .iter()
                .any(|(previous, _)| previous == id)
            {
                return Err(Error::Busy);
            }
            if let Some(completion) = block.completion(*id)? {
                let mut response = Message::new(
                    SUCCESS,
                    &[completion.status as u64, completion.bytes as u64],
                )?;
                if deliver(reply, &mut response)? {
                    block.acknowledge(*id)?;
                }
                Ok(Response::Delivered)
            } else if *wait == 1 {
                Ok(Response::Pending(*id))
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
        if let Some(completion) = block.completion(*id)? {
            if deliver(
                reply,
                &mut Message::new(
                    SUCCESS,
                    &[completion.status as u64, completion.bytes as u64],
                )?,
            )? {
                block.acknowledge(*id)?;
            }
            connection.acknowledgements.swap_remove(index);
        } else {
            index += 1;
        }
    }
    Ok(())
}

fn reject(reply: &Handle<Reply>, error: Error) -> Result<(), Error> {
    deliver(reply, &mut Message::new(session::error_status(error), &[])?)?;
    Ok(())
}

fn deliver(reply: &Handle<Reply>, message: &mut Message) -> Result<bool, Error> {
    match reply.send(message) {
        Ok(()) => Ok(true),
        Err(runtime::Error::Kernel(KernelError::InvalidState)) => Ok(false),
        Err(error) => Err(error.into()),
    }
}
