// SPDX-License-Identifier: MPL-2.0

use super::MAX_BYTES;
use crate::{
    Error,
    device::{BUSY, CHANNEL_RIGHTS, INVALID_REQUEST, OPEN, SUCCESS, UNAVAILABLE, UNSUPPORTED},
};
use runtime::{
    Channel, Handle, Rights, Task,
    ipc::{Message, ReceivedMessage, Transfer},
};

pub(super) const READ: u64 = 2;

pub struct Session<'a> {
    task: &'a Handle<Task>,
    channel: Handle<Channel>,
}

impl<'a> Session<'a> {
    pub fn open(
        task: &'a Handle<Task>,
        service: &Handle<Channel>,
        device: u32,
        deadline: u64,
    ) -> Result<Self, Error> {
        let (channel, server) = task.channel(
            CHANNEL_RIGHTS | Rights::TRANSFER,
            CHANNEL_RIGHTS | Rights::TRANSFER,
        )?;
        let mut request = Message::new(OPEN, &[device.into()])?;
        request
            .push(Transfer {
                handle: server.into(),
                rights: CHANNEL_RIGHTS | Rights::TRANSFER,
            })
            .map_err(|_| Error::InvalidRequest)?;
        let response = call(service, &mut request, deadline)?;
        if !response.words().is_empty() {
            return Err(runtime::Error::Protocol.into());
        }
        Ok(Self { task, channel })
    }

    pub fn fill(&self, mut output: &mut [u8], deadline: u64) -> Result<(), Error> {
        if output.is_empty() {
            return Ok(());
        }
        let buffer = self
            .task
            .allocate(1, Rights::READ_WRITE | Rights::DUPLICATE | Rights::TRANSFER)?;
        while !output.is_empty() {
            let length = output.len().min(MAX_BYTES);
            let mut request = Message::new(READ, &[length as u64])?;
            request
                .push(Transfer {
                    handle: buffer.try_clone(Rights::WRITE | Rights::TRANSFER)?.into(),
                    rights: Rights::WRITE,
                })
                .map_err(|_| Error::InvalidRequest)?;
            let response = call(&self.channel, &mut request, deadline)?;
            let [count] = *response.words() else {
                return Err(runtime::Error::Protocol.into());
            };
            if count == 0 || count > length as u64 {
                return Err(runtime::Error::Protocol.into());
            }
            buffer.read(0, &mut output[..count as usize])?;
            output = &mut output[count as usize..];
        }
        Ok(())
    }
}

fn call(
    channel: &Handle<Channel>,
    message: &mut Message,
    deadline: u64,
) -> Result<ReceivedMessage, Error> {
    if matches!(deadline, 0 | u64::MAX) {
        return Err(Error::InvalidRequest);
    }
    let response = channel.call(message, Some(deadline)).map_err(Error::Call)?;
    if !response.handles().is_empty() {
        return Err(runtime::Error::Protocol.into());
    }
    if response.tag == SUCCESS {
        return Ok(response);
    }
    if !response.words().is_empty() {
        return Err(runtime::Error::Protocol.into());
    }
    Err(match response.tag {
        BUSY => Error::Busy,
        INVALID_REQUEST => Error::InvalidRequest,
        UNSUPPORTED => Error::Unsupported,
        UNAVAILABLE => Error::Unavailable,
        _ => return Err(runtime::Error::Protocol.into()),
    })
}
