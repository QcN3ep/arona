// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::MAX_BYTES;
use crate::protocol::call_until as call;
use crate::protocol::response_result;
use crate::{Error, device::Pending};
use runtime::{
    Channel, Handle, Rights, Task,
    ipc::{Message, Transfer},
};

pub(super) const READ: u64 = 2;

pub type PendingOpen<'a> = Pending<Session<'a>, (&'a Handle<Task>, Handle<Channel>)>;
pub type PendingRead = Pending<alloc::vec::Vec<u8>, (Handle<runtime::FrameAllocation>, usize)>;

pub struct Session<'a> {
    task: &'a Handle<Task>,
    channel: Handle<Channel>,
}

impl<'a> Session<'a> {
    pub fn from_channel(task: &'a Handle<Task>, channel: Handle<Channel>) -> Result<Self, Error> {
        crate::device::check_session(&channel)?;
        Ok(Self { task, channel })
    }

    pub fn into_channel(self) -> Handle<Channel> {
        self.channel
    }

    pub fn channel(&self) -> &Handle<Channel> {
        &self.channel
    }

    pub fn open(
        task: &'a Handle<Task>,
        service: &Handle<Channel>,
        device: u32,
        deadline: u64,
    ) -> Result<Self, Error> {
        let deadline = crate::protocol::finite_deadline(deadline)?;
        Ok(Self {
            task,
            channel: crate::device::open(task, service, device, 0, deadline)?,
        })
    }

    pub fn start_open(
        task: &'a Handle<Task>,
        service: &Handle<Channel>,
        device: u32,
    ) -> Result<PendingOpen<'a>, Error> {
        let (call, channel) = crate::device::open_call(task, service, device)?;
        Ok(Pending::new(
            call,
            (task, channel),
            |(task, channel), response| {
                Ok(Self {
                    task,
                    channel: crate::device::complete_open(channel, response)?,
                })
            },
        ))
    }

    pub fn start_read(&self, length: usize) -> Result<PendingRead, Error> {
        if length == 0 || length > MAX_BYTES {
            return Err(Error::InvalidRequest);
        }
        let buffer = self
            .task
            .allocate(1, Rights::READ_WRITE | Rights::DUPLICATE | Rights::TRANSFER)?;
        let mut request = read_request(&buffer, length)?;
        Ok(Pending::new(
            self.channel.start_call(&mut request)?,
            (buffer, length),
            |(buffer, length), response| {
                let response = response_result(response)?;
                let count = read_count(length, &response)?;
                let mut bytes = alloc::vec![0; count];
                buffer.read(0, &mut bytes)?;
                Ok(bytes)
            },
        ))
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
            let mut request = read_request(&buffer, length)?;
            let response = call(&self.channel, &mut request, deadline)?;
            let count = read_count(length, &response)?;
            buffer.read(0, &mut output[..count])?;
            output = &mut output[count..];
        }
        Ok(())
    }
}

fn read_request(
    buffer: &Handle<runtime::FrameAllocation>,
    length: usize,
) -> Result<Message, Error> {
    let mut request = Message::new(READ, &[length as u64])?;
    request
        .push(Transfer {
            handle: buffer.try_clone(Rights::WRITE | Rights::TRANSFER)?.into(),
            rights: Rights::WRITE,
        })
        .map_err(|_| Error::InvalidRequest)?;
    Ok(request)
}

fn read_count(length: usize, response: &runtime::ipc::ReceivedMessage) -> Result<usize, Error> {
    let [count] = *response.words() else {
        return Err(runtime::Error::Protocol.into());
    };
    if count == 0 || count > length as u64 {
        return Err(runtime::Error::Protocol.into());
    }
    Ok(count as usize)
}
