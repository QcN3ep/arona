// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use runtime::{
    Call, Channel, Handle, KernelError, Rights, Task,
    ipc::{self, Message, ReceivedMessage, Transfer, WaitItem, WaitMode},
};

use super::{DataBits, LineErrors, Parity, ReadResult, Settings, StopBits};
use crate::{
    Error,
    device::{
        BUSY, CHANNEL_RIGHTS, CLOCK_UNAVAILABLE, DENIED, INVALID_CONFIGURATION, INVALID_REQUEST,
        OPEN, Pending, SUCCESS, UNAVAILABLE, UNSUPPORTED,
    },
};

pub(super) const READ: u64 = 2;
pub(super) const TRY_READ: u64 = 3;
pub(super) const WRITE: u64 = 4;
pub(super) const FLUSH: u64 = 5;
pub(super) const CONFIGURE: u64 = 6;
pub(super) const SET_ACCESS: u64 = 7;
pub const MAX_BYTES: usize = (runtime::ipc::MESSAGE_WORDS - 2) * 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u64)]
pub enum Access {
    InputOutput = 1,
    Output = 2,
}

pub type PendingRead = Pending<(alloc::vec::Vec<u8>, LineErrors), usize>;

pub struct Session {
    channel: Handle<Channel>,
}

pub struct PendingOpen {
    call: Handle<Call>,
    stage: OpenStage,
}

enum OpenStage {
    Open(Handle<Channel>, Access),
    Configure(Handle<Channel>),
    Finished,
}

impl PendingOpen {
    pub fn call(&self) -> &Handle<Call> {
        &self.call
    }

    pub fn cancel(&self) -> Result<(), Error> {
        Ok(self.call.cancel()?)
    }

    pub fn wait(&mut self, mode: WaitMode) -> Result<Session, Error> {
        loop {
            ipc::wait_many(&[WaitItem::new(&self.call, ipc::READABLE)], mode)?;
            match self.try_complete() {
                Err(Error::Runtime(runtime::Error::Kernel(KernelError::WouldBlock))) => {}
                result => return result,
            }
        }
    }

    pub fn try_complete(&mut self) -> Result<Session, Error> {
        let response = self.call.try_receive()?;
        match core::mem::replace(&mut self.stage, OpenStage::Finished) {
            OpenStage::Open(channel, access) => {
                let channel = crate::device::complete_open(channel, response)?;
                self.call = channel.start_call(&mut Message::new(SET_ACCESS, &[access as u64])?)?;
                self.stage = OpenStage::Configure(channel);
                Err(runtime::Error::Kernel(KernelError::WouldBlock).into())
            }
            OpenStage::Configure(channel) => {
                empty(response_result(response)?)?;
                Ok(Session { channel })
            }
            OpenStage::Finished => Err(runtime::Error::Kernel(KernelError::InvalidState).into()),
        }
    }
}

impl Session {
    pub fn channel(&self) -> &Handle<Channel> {
        &self.channel
    }

    pub fn open(
        task: &Handle<Task>,
        service: &Handle<Channel>,
        device: u32,
        access: Access,
    ) -> Result<Self, Error> {
        let (channel, server) = task.channel(
            CHANNEL_RIGHTS.union(Rights::TRANSFER),
            CHANNEL_RIGHTS.union(Rights::TRANSFER),
        )?;
        let mut request = Message::new(OPEN, &[u64::from(device)])?;
        request
            .push(Transfer {
                handle: server.into(),
                rights: CHANNEL_RIGHTS.union(Rights::TRANSFER),
            })
            .map_err(|_| runtime::Error::Protocol)?;
        empty(response(service, &mut request)?)?;
        empty(response(
            &channel,
            &mut Message::new(SET_ACCESS, &[access as u64])?,
        )?)?;
        Ok(Self { channel })
    }

    pub fn from_channel(channel: Handle<Channel>) -> Result<Self, Error> {
        if !channel.info()?.rights.contains(CHANNEL_RIGHTS) {
            return Err(runtime::Error::Kernel(runtime::KernelError::Denied).into());
        }
        Ok(Self { channel })
    }

    pub fn into_channel(self) -> Handle<Channel> {
        self.channel
    }

    pub fn start_open(
        task: &Handle<Task>,
        service: &Handle<Channel>,
        device: u32,
        access: Access,
    ) -> Result<PendingOpen, Error> {
        let (call, channel) = crate::device::open_call(task, service, device)?;
        Ok(PendingOpen {
            call,
            stage: OpenStage::Open(channel, access),
        })
    }

    pub fn start_read(&self, length: usize) -> Result<PendingRead, Error> {
        self.start_receive(length, READ)
    }

    pub fn start_try_read(&self, length: usize) -> Result<PendingRead, Error> {
        self.start_receive(length, TRY_READ)
    }

    pub fn start_write(&self, bytes: &[u8]) -> Result<Pending<usize, usize>, Error> {
        let bytes = &bytes[..bytes.len().min(MAX_BYTES)];
        let words = encode(bytes);
        let call = self.channel.start_call(&mut Message::new(
            WRITE,
            &words[..1 + bytes.len().div_ceil(8)],
        )?)?;
        Ok(Pending::new(call, bytes.len(), |length, response| {
            let response = response_result(response)?;
            if response.words() != [length as u64] {
                return Err(runtime::Error::Protocol.into());
            }
            Ok(length)
        }))
    }

    pub fn start_flush(&self) -> Result<Pending<()>, Error> {
        self.start_empty(&mut Message::new(FLUSH, &[])?)
    }

    pub fn start_configure(&self, settings: Settings) -> Result<Pending<()>, Error> {
        self.start_empty(&mut Message::new(CONFIGURE, &settings_words(settings)?)?)
    }

    fn start_receive(&self, length: usize, method: u64) -> Result<PendingRead, Error> {
        if length == 0 {
            return Err(Error::InvalidRequest);
        }
        let length = length.min(MAX_BYTES);
        let call = self
            .channel
            .start_call(&mut Message::new(method, &[length as u64])?)?;
        Ok(Pending::new(call, length, |length, response| {
            let (bytes, result) = read_result(length, response_result(response)?)?;
            Ok((bytes[..result.bytes].to_vec(), result.errors))
        }))
    }

    fn start_empty(&self, message: &mut Message) -> Result<Pending<()>, Error> {
        Ok(Pending::new(
            self.channel.start_call(message)?,
            (),
            |(), response| empty(response_result(response)?),
        ))
    }

    pub fn read(&self, output: &mut [u8]) -> Result<ReadResult, Error> {
        self.receive(output, READ)
    }

    pub fn try_read(&self, output: &mut [u8]) -> Result<ReadResult, Error> {
        self.receive(output, TRY_READ)
    }

    /// Returns the number of bytes accepted into the transmit buffer.
    pub fn write(&self, bytes: &[u8]) -> Result<usize, Error> {
        let bytes = &bytes[..bytes.len().min(MAX_BYTES)];
        let words = encode(bytes);
        let mut request = Message::new(WRITE, &words[..1 + bytes.len().div_ceil(8)])?;
        let response = response(&self.channel, &mut request)?;
        if response.words() != [bytes.len() as u64] {
            return Err(runtime::Error::Protocol.into());
        }
        Ok(bytes.len())
    }

    pub fn write_all(&self, mut bytes: &[u8]) -> Result<(), Error> {
        while !bytes.is_empty() {
            let written = self.write(bytes)?;
            bytes = &bytes[written..];
        }
        Ok(())
    }

    pub fn flush(&self) -> Result<(), Error> {
        empty(response(&self.channel, &mut Message::new(FLUSH, &[])?)?)
    }

    pub fn configure(&self, settings: Settings) -> Result<(), Error> {
        let words = settings_words(settings)?;
        empty(response(
            &self.channel,
            &mut Message::new(CONFIGURE, &words)?,
        )?)
    }

    fn receive(&self, output: &mut [u8], operation: u64) -> Result<ReadResult, Error> {
        if output.is_empty() {
            return Ok(ReadResult {
                bytes: 0,
                errors: LineErrors::default(),
            });
        }
        let length = output.len().min(MAX_BYTES);
        let response = response(
            &self.channel,
            &mut Message::new(operation, &[length as u64])?,
        )?;
        let (bytes, result) = read_result(length, response)?;
        output[..result.bytes].copy_from_slice(&bytes[..result.bytes]);
        Ok(result)
    }
}

pub(super) fn encode(bytes: &[u8]) -> [u64; runtime::ipc::MESSAGE_WORDS] {
    let mut words = [0; runtime::ipc::MESSAGE_WORDS];
    words[0] = bytes.len() as u64;
    for (index, byte) in bytes.iter().enumerate() {
        words[1 + index / 8] |= u64::from(*byte) << ((index % 8) * 8);
    }
    words
}

pub(super) fn error_bits(errors: LineErrors) -> u64 {
    u64::from(errors.overrun)
        | (u64::from(errors.parity) << 1)
        | (u64::from(errors.framing) << 2)
        | (u64::from(errors.break_condition) << 3)
}

fn line_errors(bits: u64) -> LineErrors {
    LineErrors {
        overrun: bits & 1 != 0,
        parity: bits & 2 != 0,
        framing: bits & 4 != 0,
        break_condition: bits & 8 != 0,
    }
}

fn response(channel: &Handle<Channel>, request: &mut Message) -> Result<ReceivedMessage, Error> {
    let response = channel.call(request, None).map_err(Error::Call)?;
    response_result(response)
}

fn response_result(response: ReceivedMessage) -> Result<ReceivedMessage, Error> {
    if !response.handles().is_empty() || response.tag != SUCCESS && !response.words().is_empty() {
        return Err(runtime::Error::Protocol.into());
    }
    let error = match response.tag {
        SUCCESS => return Ok(response),
        INVALID_REQUEST => runtime::Error::Protocol.into(),
        BUSY => Error::Busy,
        DENIED => Error::Denied,
        UNSUPPORTED => Error::Unsupported,
        INVALID_CONFIGURATION => Error::InvalidConfiguration,
        CLOCK_UNAVAILABLE => Error::ClockUnavailable,
        UNAVAILABLE => Error::Unavailable,
        _ => runtime::Error::Protocol.into(),
    };
    Err(error)
}

fn empty(response: ReceivedMessage) -> Result<(), Error> {
    if response.words().is_empty() {
        Ok(())
    } else {
        Err(runtime::Error::Protocol.into())
    }
}

fn settings_words(settings: Settings) -> Result<[u64; 4], Error> {
    let words = [
        u64::from(settings.baud_rate.unwrap_or(0)),
        match settings.data_bits {
            DataBits::Five => 5,
            DataBits::Six => 6,
            DataBits::Seven => 7,
            DataBits::Eight => 8,
        },
        match settings.parity {
            Parity::None => 0,
            Parity::Odd => 1,
            Parity::Even => 2,
        },
        match settings.stop_bits {
            StopBits::One => 0,
            StopBits::OneAndHalf => 1,
            StopBits::Two => 2,
        },
    ];
    if settings.baud_rate == Some(0) {
        return Err(Error::InvalidConfiguration);
    }
    Ok(words)
}

fn read_result(
    length: usize,
    response: ReceivedMessage,
) -> Result<([u8; MAX_BYTES], ReadResult), Error> {
    let words = response.words();
    let [count, errors, data @ ..] = words else {
        return Err(runtime::Error::Protocol.into());
    };
    let count = usize::try_from(*count).map_err(|_| runtime::Error::Protocol)?;
    if count > length || data.len() != count.div_ceil(8) || *errors & !15 != 0 {
        return Err(runtime::Error::Protocol.into());
    }
    let mut output = [0; MAX_BYTES];
    for (index, byte) in output[..count].iter_mut().enumerate() {
        *byte = (data[index / 8] >> ((index % 8) * 8)) as u8;
    }
    Ok((
        output,
        ReadResult {
            bytes: count,
            errors: line_errors(*errors),
        },
    ))
}
