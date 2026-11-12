// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::{Info, MAX_FRAME, MIN_FRAME};
use crate::{
    Error,
    device::{self, BUSY, CHANNEL_RIGHTS, INVALID_REQUEST, Pending, SUCCESS, UNAVAILABLE},
};
use runtime::{
    Channel, FrameAllocation, Handle, Rights, Task,
    ipc::{Message, ReceivedMessage, Transfer, WaitMode},
};

pub(super) const INFO: u64 = 2;
pub(super) const RECEIVE: u64 = 3;
pub(super) const TRANSMIT: u64 = 4;
pub(super) const WATCH: u64 = 5;

pub type PendingOpen<'a> = Pending<Session<'a>, (&'a Handle<Task>, Handle<Channel>)>;
pub type PendingReceive = Pending<alloc::vec::Vec<u8>, Handle<FrameAllocation>>;

/// Exclusive frame access without register or DMA capabilities.
/// At most one receive and one link watch may wait at a time.
/// Canceling a transmit reply does not recall an already published frame.
pub struct Session<'a> {
    task: &'a Handle<Task>,
    channel: Handle<Channel>,
}

impl<'a> Session<'a> {
    pub fn from_channel(task: &'a Handle<Task>, channel: Handle<Channel>) -> Result<Self, Error> {
        if !channel.info()?.rights.contains(CHANNEL_RIGHTS) {
            return Err(Error::Denied);
        }
        Ok(Self { task, channel })
    }

    pub fn into_channel(self) -> Handle<Channel> {
        self.channel
    }
    pub fn channel(&self) -> &Handle<Channel> {
        &self.channel
    }

    pub fn start_open(
        task: &'a Handle<Task>,
        service: &Handle<Channel>,
        device: u32,
    ) -> Result<PendingOpen<'a>, Error> {
        let (call, channel) = device::open_call(task, service, device)?;
        Ok(Pending::new(
            call,
            (task, channel),
            |(task, channel), response| {
                Ok(Self {
                    task,
                    channel: device::complete_open(channel, response)?,
                })
            },
        ))
    }

    pub fn open(
        task: &'a Handle<Task>,
        service: &Handle<Channel>,
        device: u32,
        deadline: u64,
    ) -> Result<Self, Error> {
        Self::start_open(task, service, device)?.wait(WaitMode::Until(deadline))
    }

    pub fn start_info(&self) -> Result<Pending<Info>, Error> {
        Ok(Pending::new(
            self.channel.start_call(&mut Message::new(INFO, &[])?)?,
            (),
            |(), response| decode_info(response),
        ))
    }

    /// Wait for a different link sequence. Query info first, then pass its sequence.
    pub fn start_watch(&self, sequence: u64) -> Result<Pending<Info>, Error> {
        Ok(Pending::new(
            self.channel
                .start_call(&mut Message::new(WATCH, &[sequence])?)?,
            (),
            |(), response| decode_info(response),
        ))
    }

    /// Wait for one complete frame. Canceled replies leave the frame available.
    pub fn start_receive(&self) -> Result<PendingReceive, Error> {
        let buffer = self
            .task
            .allocate(1, Rights::READ_WRITE | Rights::DUPLICATE | Rights::TRANSFER)?;
        let mut message = Message::new(RECEIVE, &[])?;
        message
            .push(Transfer {
                handle: buffer.try_clone(Rights::WRITE | Rights::TRANSFER)?.into(),
                rights: Rights::WRITE,
            })
            .map_err(|_| Error::InvalidRequest)?;
        Ok(Pending::new(
            self.channel.start_call(&mut message)?,
            buffer,
            |buffer, response| {
                let response = response_result(response)?;
                let [length] = *response.words() else {
                    return Err(runtime::Error::Protocol.into());
                };
                if !(MIN_FRAME as u64..=MAX_FRAME as u64).contains(&length) {
                    return Err(runtime::Error::Protocol.into());
                }
                let mut frame = alloc::vec![0; length as usize];
                buffer.read(0, &mut frame)?;
                Ok(frame)
            },
        ))
    }

    /// Submit a copied frame. Success means device completion, not peer delivery.
    /// Busy means no frame was published. After submission, cancellation or a lost
    /// reply cannot establish whether a frame was transmitted.
    pub fn start_transmit(&self, frame: &[u8]) -> Result<Pending<()>, Error> {
        if !(MIN_FRAME..=MAX_FRAME).contains(&frame.len()) {
            return Err(Error::InvalidRequest);
        }
        let mut buffer = self
            .task
            .allocate(1, Rights::READ_WRITE | Rights::TRANSFER)?;
        buffer.write(0, frame)?;
        let mut message = Message::new(TRANSMIT, &[frame.len() as u64])?;
        message
            .push(Transfer {
                handle: buffer.into(),
                rights: Rights::READ,
            })
            .map_err(|_| Error::InvalidRequest)?;
        Ok(Pending::new(
            self.channel.start_call(&mut message)?,
            (),
            |(), response| {
                if !response_result(response)?.words().is_empty() {
                    return Err(runtime::Error::Protocol.into());
                }
                Ok(())
            },
        ))
    }
}

fn decode_info(response: ReceivedMessage) -> Result<Info, Error> {
    let response = response_result(response)?;
    let [mac, mtu, link, sequence] = *response.words() else {
        return Err(runtime::Error::Protocol.into());
    };
    if mac >> 48 != 0 || !(68..=1500).contains(&mtu) || link > 1 {
        return Err(runtime::Error::Protocol.into());
    }
    let mac: [u8; 6] = mac.to_le_bytes()[..6].try_into().unwrap();
    if mac[0] & 1 != 0 || mac == [0; 6] {
        return Err(runtime::Error::Protocol.into());
    }
    Ok(Info {
        mac,
        mtu: mtu as u16,
        link_up: link != 0,
        sequence,
    })
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
        UNAVAILABLE => Error::Unavailable,
        _ => return Err(runtime::Error::Protocol.into()),
    })
}
