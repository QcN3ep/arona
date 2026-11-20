// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::Timestamp;
use crate::protocol::call_until as call;
use crate::protocol::response_result;
use crate::{Error, device::Pending};
use runtime::{
    Channel, Handle, Task,
    ipc::{Message, ReceivedMessage},
};

pub(super) const NOW: u64 = 2;
pub(super) const SET: u64 = 3;

pub struct Session {
    channel: Handle<Channel>,
}

impl Session {
    pub fn from_channel(channel: Handle<Channel>) -> Result<Self, Error> {
        crate::device::check_session(&channel)?;
        Ok(Self { channel })
    }

    pub fn into_channel(self) -> Handle<Channel> {
        self.channel
    }

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

    pub fn start_open(
        task: &Handle<Task>,
        service: &Handle<Channel>,
        device: u32,
    ) -> Result<Pending<Self, Handle<Channel>>, Error> {
        let (call, channel) = crate::device::open_call(task, service, device)?;
        Ok(Pending::new(call, channel, |channel, response| {
            Ok(Self {
                channel: crate::device::complete_open(channel, response)?,
            })
        }))
    }

    pub fn start_now(&self) -> Result<Pending<Timestamp>, Error> {
        Ok(Pending::new(
            self.channel.start_call(&mut Message::new(NOW, &[])?)?,
            (),
            |(), response| timestamp(response_result(response)?),
        ))
    }

    pub fn now(&self, deadline: u64) -> Result<Timestamp, Error> {
        let response = call(&self.channel, &mut Message::new(NOW, &[])?, deadline)?;
        timestamp(response)
    }

    pub fn start_set(&self, time: Timestamp) -> Result<Pending<()>, Error> {
        Ok(Pending::new(
            self.channel.start_call(&mut set_message(time)?)?,
            (),
            |(), response| {
                if !response_result(response)?.words().is_empty() {
                    return Err(runtime::Error::Protocol.into());
                }
                Ok(())
            },
        ))
    }

    pub fn set(&self, time: Timestamp, deadline: u64) -> Result<(), Error> {
        let response = call(&self.channel, &mut set_message(time)?, deadline)?;
        if !response.words().is_empty() {
            return Err(runtime::Error::Protocol.into());
        }
        Ok(())
    }
}

fn set_message(time: Timestamp) -> Result<Message, Error> {
    Ok(Message::new(
        SET,
        &[time.seconds as u64, time.nanoseconds.into()],
    )?)
}

fn timestamp(response: ReceivedMessage) -> Result<Timestamp, Error> {
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
