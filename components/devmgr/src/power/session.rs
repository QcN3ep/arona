// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Client sessions for platform power capability queries and actions.

use super::Capabilities;
use crate::Error;
use crate::protocol::call_until as call;
use crate::protocol::empty;
use runtime::{
    Channel, Handle, Task,
    ipc::{Message, ReceivedMessage},
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
        let deadline = crate::protocol::finite_deadline(deadline)?;
        Ok(Self {
            channel: crate::device::open(task, service, device, 0, deadline)?,
        })
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
