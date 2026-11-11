// SPDX-License-Identifier: MPL-2.0

//! Request submission, cancellation, and completion acknowledgement for block device sessions.

use runtime::{
    Channel, FrameAllocation, Handle, Rights, Task,
    ipc::{Message, ReceivedMessage, Transfer},
};

use super::{CancelResult, Completion, Info, Status};
use crate::{
    Error,
    device::{
        BUSY, CHANNEL_RIGHTS, INVALID_REQUEST, OPEN, Pending, SUCCESS, UNAVAILABLE, UNSUPPORTED,
    },
};

pub(super) const INFO: u64 = 2;
pub(super) const READ: u64 = 3;
pub(super) const WRITE: u64 = 4;
pub(super) const FLUSH: u64 = 5;
pub(super) const CANCEL: u64 = 6;
pub(super) const ACK: u64 = 7;
const READ_ONLY: u64 = 7;
const NOT_FOUND: u64 = 8;
const DEVICE_FAULT: u64 = 9;

pub struct Session {
    channel: Handle<Channel>,
}

impl Session {
    pub fn channel(&self) -> &Handle<Channel> {
        &self.channel
    }

    pub fn from_channel(channel: Handle<Channel>) -> Result<Self, Error> {
        let session = Self { channel };
        session.info()?;
        Ok(session)
    }

    pub fn into_channel(self) -> Handle<Channel> {
        self.channel
    }

    pub fn open(
        task: &Handle<Task>,
        service: &Handle<Channel>,
        device: u32,
    ) -> Result<Self, Error> {
        let rights = CHANNEL_RIGHTS.union(Rights::TRANSFER);
        let (channel, server) = task.channel(rights, rights)?;
        let mut request = Message::new(OPEN, &[device.into()])?;
        request
            .push(Transfer {
                handle: server.into(),
                rights,
            })
            .map_err(|_| runtime::Error::Protocol)?;
        empty(response(service, &mut request, None)?)?;
        Ok(Self { channel })
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

    pub fn start_info(&self) -> Result<Pending<Info>, Error> {
        Ok(Pending::new(
            self.channel.start_call(&mut Message::new(INFO, &[])?)?,
            (),
            |(), response| info(response_result(response)?),
        ))
    }

    pub fn start_read(
        &self,
        id: u64,
        offset: u64,
        output: &Handle<FrameAllocation>,
        buffer_offset: usize,
        length: usize,
    ) -> Result<Pending<()>, Error> {
        self.start_transfer(READ, id, offset, output, buffer_offset, length)
    }

    pub fn start_write(
        &self,
        id: u64,
        offset: u64,
        input: &Handle<FrameAllocation>,
        buffer_offset: usize,
        length: usize,
    ) -> Result<Pending<()>, Error> {
        self.start_transfer(WRITE, id, offset, input, buffer_offset, length)
    }

    pub fn start_flush(&self, id: u64) -> Result<Pending<()>, Error> {
        self.start_empty(&mut Message::new(FLUSH, &[id])?)
    }

    pub fn start_cancel(&self, id: u64) -> Result<Pending<CancelResult>, Error> {
        Ok(Pending::new(
            self.channel.start_call(&mut Message::new(CANCEL, &[id])?)?,
            (),
            |(), response| cancellation(response_result(response)?),
        ))
    }

    pub fn start_acknowledge(&self, id: u64) -> Result<Pending<Completion>, Error> {
        Ok(Pending::new(
            self.channel.start_call(&mut Message::new(ACK, &[id, 1])?)?,
            (),
            |(), response| completion(response_result(response)?),
        ))
    }

    fn start_empty(&self, request: &mut Message) -> Result<Pending<()>, Error> {
        Ok(Pending::new(
            self.channel.start_call(request)?,
            (),
            |(), response| empty(response_result(response)?),
        ))
    }

    fn start_transfer(
        &self,
        method: u64,
        id: u64,
        offset: u64,
        frames: &Handle<FrameAllocation>,
        buffer_offset: usize,
        length: usize,
    ) -> Result<Pending<()>, Error> {
        self.start_empty(&mut transfer_request(
            method,
            id,
            offset,
            frames,
            buffer_offset,
            length,
        )?)
    }

    pub fn info(&self) -> Result<Info, Error> {
        let response = response(&self.channel, &mut Message::new(INFO, &[])?, None)?;
        info(response)
    }

    pub fn read(
        &self,
        id: u64,
        offset: u64,
        output: &Handle<FrameAllocation>,
        buffer_offset: usize,
        length: usize,
    ) -> Result<(), Error> {
        self.transfer(READ, id, offset, output, buffer_offset, length)
    }

    pub fn write(
        &self,
        id: u64,
        offset: u64,
        input: &Handle<FrameAllocation>,
        buffer_offset: usize,
        length: usize,
    ) -> Result<(), Error> {
        self.transfer(WRITE, id, offset, input, buffer_offset, length)
    }

    pub fn flush(&self, id: u64) -> Result<(), Error> {
        empty(response(
            &self.channel,
            &mut Message::new(FLUSH, &[id])?,
            None,
        )?)
    }

    pub fn cancel(&self, id: u64) -> Result<CancelResult, Error> {
        let response = response(&self.channel, &mut Message::new(CANCEL, &[id])?, None)?;
        cancellation(response)
    }

    pub fn acknowledge(&self, id: u64, deadline: Option<u64>) -> Result<Completion, Error> {
        self.acknowledge_mode(id, true, deadline)
    }

    pub fn try_acknowledge(&self, id: u64) -> Result<Completion, Error> {
        self.acknowledge_mode(id, false, None)
    }

    fn acknowledge_mode(
        &self,
        id: u64,
        wait: bool,
        deadline: Option<u64>,
    ) -> Result<Completion, Error> {
        let response = response(
            &self.channel,
            &mut Message::new(ACK, &[id, u64::from(wait)])?,
            deadline,
        )?;
        completion(response)
    }

    fn transfer(
        &self,
        method: u64,
        id: u64,
        offset: u64,
        frames: &Handle<FrameAllocation>,
        buffer_offset: usize,
        length: usize,
    ) -> Result<(), Error> {
        let mut request = transfer_request(method, id, offset, frames, buffer_offset, length)?;
        empty(response(&self.channel, &mut request, None)?)
    }
}

pub(super) fn error_status(error: Error) -> u64 {
    match error {
        Error::Busy => BUSY,
        Error::Unsupported => UNSUPPORTED,
        Error::ReadOnly => READ_ONLY,
        Error::NotFound => NOT_FOUND,
        Error::DeviceFault => DEVICE_FAULT,
        Error::InvalidRequest => INVALID_REQUEST,
        _ => UNAVAILABLE,
    }
}

fn response(
    channel: &Handle<Channel>,
    request: &mut Message,
    deadline: Option<u64>,
) -> Result<ReceivedMessage, Error> {
    let response = channel.call(request, deadline).map_err(Error::Call)?;
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
        INVALID_REQUEST => Error::InvalidRequest,
        BUSY => Error::Busy,
        UNSUPPORTED => Error::Unsupported,
        READ_ONLY => Error::ReadOnly,
        NOT_FOUND => Error::NotFound,
        DEVICE_FAULT => Error::DeviceFault,
        UNAVAILABLE => Error::Unavailable,
        _ => return Err(runtime::Error::Protocol.into()),
    })
}

fn empty(response: ReceivedMessage) -> Result<(), Error> {
    if response.words().is_empty() {
        Ok(())
    } else {
        Err(runtime::Error::Protocol.into())
    }
}

fn info(response: ReceivedMessage) -> Result<Info, Error> {
    let [capacity, block_size, flags, max_transfer, max_requests] = *response.words() else {
        return Err(runtime::Error::Protocol.into());
    };
    if flags & !3 != 0
        || !block_size.is_power_of_two()
        || !(512..=4096).contains(&block_size)
        || capacity == 0
        || !capacity.is_multiple_of(block_size)
        || max_transfer < block_size
        || max_transfer > super::MAX_TRANSFER as u64
        || max_requests == 0
        || max_requests > super::MAX_REQUESTS as u64
    {
        return Err(runtime::Error::Protocol.into());
    }
    Ok(Info {
        capacity,
        block_size: block_size as u32,
        read_only: flags & 1 != 0,
        flush: flags & 2 != 0,
        max_transfer: max_transfer as usize,
        max_requests: max_requests as usize,
    })
}

fn cancellation(response: ReceivedMessage) -> Result<CancelResult, Error> {
    match response.words() {
        [0] => Ok(CancelResult::Cancelled),
        [1] => Ok(CancelResult::Submitted),
        [2] => Ok(CancelResult::Completed),
        _ => Err(runtime::Error::Protocol.into()),
    }
}

fn completion(response: ReceivedMessage) -> Result<Completion, Error> {
    let [status, bytes] = *response.words() else {
        return Err(runtime::Error::Protocol.into());
    };
    let status = match status {
        0 => Status::Success,
        1 => Status::Io,
        2 => Status::Unsupported,
        3 => Status::Cancelled,
        _ => return Err(runtime::Error::Protocol.into()),
    };
    if bytes > super::MAX_TRANSFER as u64 {
        return Err(runtime::Error::Protocol.into());
    }
    Ok(Completion {
        status,
        bytes: bytes as usize,
    })
}

fn transfer_request(
    method: u64,
    id: u64,
    offset: u64,
    frames: &Handle<FrameAllocation>,
    buffer_offset: usize,
    length: usize,
) -> Result<Message, Error> {
    let rights = if method == READ {
        Rights::WRITE
    } else {
        Rights::READ
    };
    let mut request = Message::new(method, &[id, offset, buffer_offset as u64, length as u64])?;
    request
        .push(Transfer {
            handle: frames.try_clone(rights.union(Rights::TRANSFER))?.into(),
            rights,
        })
        .map_err(|_| runtime::Error::Protocol)?;
    Ok(request)
}
