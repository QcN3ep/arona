// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Client sessions for platform power capability queries and actions.

use super::Capabilities;
use crate::{
    Error,
    device::{BUSY, CHANNEL_RIGHTS, INVALID_REQUEST, OPEN, SUCCESS, UNAVAILABLE, UNSUPPORTED},
};
use runtime::{
    Channel, Handle, Rights, Task,
    ipc::{Message, ReceivedMessage, Transfer},
};

pub(super) const CAPABILITIES: u64 = 2;
pub(super) const SHUTDOWN: u64 = 3;
pub(super) const REBOOT: u64 = 4;

/// A client session with a system controller power instance.
pub struct Session {
    channel: Handle<Channel>,
}

impl Session {
    pub fn channel(&self) -> &Handle<Channel> {
        &self.channel
    }

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

    pub fn capabilities(&self, deadline: u64) -> Result<Capabilities, Error> {
        let response = call(
            &self.channel,
            &mut Message::new(CAPABILITIES, &[])?,
            deadline,
        )?;
        capabilities(response)
    }

    pub fn shutdown(&self, deadline: u64) -> Result<(), Error> {
        self.perform(SHUTDOWN, deadline)
    }

    pub fn reboot(&self, deadline: u64) -> Result<(), Error> {
        self.perform(REBOOT, deadline)
    }

    fn perform(&self, tag: u64, deadline: u64) -> Result<(), Error> {
        let response = call(&self.channel, &mut Message::new(tag, &[])?, deadline)?;
        empty(response)
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
    response_result(response)
}

fn response_result(response: ReceivedMessage) -> Result<ReceivedMessage, Error> {
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

fn capabilities(response: ReceivedMessage) -> Result<Capabilities, Error> {
    let [shutdown, reboot] = *response.words() else {
        return Err(runtime::Error::Protocol.into());
    };
    let flag = |value: u64| -> Result<bool, Error> {
        match value {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(runtime::Error::Protocol.into()),
        }
    };
    Ok(Capabilities {
        shutdown: flag(shutdown)?,
        reboot: flag(reboot)?,
    })
}

fn empty(response: ReceivedMessage) -> Result<(), Error> {
    if response.words().is_empty() {
        Ok(())
    } else {
        Err(runtime::Error::Protocol.into())
    }
}
