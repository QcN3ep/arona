// SPDX-License-Identifier: MPL-2.0

use super::Timestamp;
use crate::{
    Error,
    device::{BUSY, CHANNEL_RIGHTS, INVALID_REQUEST, OPEN, SUCCESS, UNAVAILABLE, UNSUPPORTED},
};
use runtime::{
    Channel, Handle, Rights, Task,
    ipc::{Message, ReceivedMessage, Transfer},
};

pub(super) const NOW: u64 = 2;

pub struct Session {
    channel: Handle<Channel>,
}

impl Session {
    pub fn open(
        task: &Handle<Task>,
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
        Ok(Self { channel })
    }

    pub fn now(&self, deadline: u64) -> Result<Timestamp, Error> {
        let response = call(&self.channel, &mut Message::new(NOW, &[])?, deadline)?;
        let [seconds, nanoseconds] = *response.words() else {
            return Err(runtime::Error::Protocol.into());
        };
        if nanoseconds >= 1_000_000_000 {
            return Err(runtime::Error::Protocol.into());
        }
        Ok(Timestamp {
            seconds: seconds as i64,
            nanoseconds: nanoseconds as u32,
        })
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
