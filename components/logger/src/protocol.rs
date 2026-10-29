// SPDX-License-Identifier: MPL-2.0

use arrayvec::ArrayString;
use core::fmt::{self, Write};
use runtime::{
    Channel, FrameAllocation, Handle, Rights, Task,
    ipc::{Message, ReceivedMessage, Transfer},
};

use crate::{Error, Level, MAX_RECORD_SIZE};

pub const START: u64 = 0;
pub const WRITE: u64 = 1;
pub const FLUSH: u64 = 2;
pub const READ: u64 = 16;
pub const READ_KERNEL: u64 = 17;
pub const PRINT_KERNEL: u64 = 18;
pub const SUCCESS: u64 = 0;
pub const INVALID_REQUEST: u64 = 1;
pub const BUFFER_TOO_SMALL: u64 = 2;
pub const UNAVAILABLE: u64 = 3;
pub const BUSY: u64 = 4;
pub const CHANNEL_RIGHTS: Rights = Rights::READ_WRITE.union(Rights::WAIT);
pub const MAX_TEXT: usize = MAX_RECORD_SIZE - abi::logger::HEADER_SIZE;

pub struct Submission {
    pub text: ArrayString<MAX_TEXT>,
    pub target_length: usize,
    pub level: Level,
    pub truncated: bool,
}

impl Submission {
    pub fn new(level: log::Level, target: &str, message: fmt::Arguments<'_>) -> Self {
        let mut this = Self {
            text: ArrayString::new_const(),
            target_length: 0,
            level: match level {
                log::Level::Error => Level::Error,
                log::Level::Warn => Level::Warn,
                log::Level::Info => Level::Info,
                log::Level::Debug => Level::Debug,
                log::Level::Trace => Level::Trace,
            },
            truncated: false,
        };
        let _ = this.write_str(target);
        this.target_length = this.text.len();
        if fmt::write(&mut this, message).is_err() {
            this.truncated = true;
        }
        this
    }

    pub fn encode(&self, task: &Handle<Task>) -> Result<Message, Error> {
        let header = self.level as u64
            | u64::from(self.truncated) << 3
            | (self.target_length as u64) << 16
            | ((self.text.len() - self.target_length) as u64) << 32;
        if self.text.len() <= (runtime::ipc::MESSAGE_WORDS - 1) * 8 {
            let mut words = [0; runtime::ipc::MESSAGE_WORDS];
            words[0] = header;
            for (index, byte) in self.text.bytes().enumerate() {
                words[1 + index / 8] |= u64::from(byte) << (index % 8 * 8);
            }
            Ok(Message::new(
                WRITE,
                &words[..1 + self.text.len().div_ceil(8)],
            )?)
        } else {
            let mut frames = task.allocate(1, Rights::READ_WRITE | Rights::TRANSFER)?;
            frames.write(0, self.text.as_bytes())?;
            let mut message = Message::new(WRITE, &[header])?;
            transfer(&mut message, frames, Rights::READ)?;
            Ok(message)
        }
    }

    pub fn decode(mut message: ReceivedMessage) -> Result<Self, Error> {
        let (&header, data) = message.words().split_first().ok_or(Error::InvalidRequest)?;
        if header & !0x0000_ffff_ffff_000f != 0 {
            return Err(Error::InvalidRequest);
        }
        let level = match header & 7 {
            1 => Level::Error,
            2 => Level::Warn,
            3 => Level::Info,
            4 => Level::Debug,
            5 => Level::Trace,
            _ => return Err(Error::InvalidRequest),
        };
        let target_length = ((header >> 16) & 0xffff) as usize;
        let length = target_length + ((header >> 32) & 0xffff) as usize;
        if length > MAX_TEXT {
            return Err(Error::InvalidRequest);
        }
        let mut bytes = [0; MAX_TEXT];
        if message.handles().is_empty() && data.len() == length.div_ceil(8) {
            for (index, byte) in bytes[..length].iter_mut().enumerate() {
                *byte = (data[index / 8] >> (index % 8 * 8)) as u8;
            }
        } else if data.is_empty() && message.handles().len() == 1 {
            message
                .take_handle(0)
                .unwrap()
                .downcast::<FrameAllocation>()
                .map_err(|_| Error::InvalidRequest)?
                .read(0, &mut bytes[..length])?;
        } else {
            return Err(Error::InvalidRequest);
        }
        let text = core::str::from_utf8(&bytes[..length]).map_err(|_| Error::InvalidRequest)?;
        if !text.is_char_boundary(target_length) {
            return Err(Error::InvalidRequest);
        }
        Ok(Self {
            text: ArrayString::from(text).map_err(|_| Error::InvalidRequest)?,
            target_length,
            level,
            truncated: header & 8 != 0,
        })
    }
}

impl Write for Submission {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let length =
            text.floor_char_boundary((self.text.capacity() - self.text.len()).min(text.len()));
        self.text.push_str(&text[..length]);
        if length < text.len() {
            self.truncated = true;
            Err(fmt::Error)
        } else {
            Ok(())
        }
    }
}

pub fn transfer<T: runtime::Object>(
    message: &mut Message,
    handle: Handle<T>,
    rights: Rights,
) -> Result<(), Error> {
    message
        .push(Transfer {
            handle: handle.into(),
            rights,
        })
        .map_err(|_| Error::InvalidRequest)
}

pub fn response(response: ReceivedMessage) -> Result<ReceivedMessage, Error> {
    if !response.handles().is_empty() {
        return Err(Error::InvalidRequest);
    }
    if response.tag == SUCCESS {
        return Ok(response);
    }
    if !response.words().is_empty() {
        return Err(Error::InvalidRequest);
    }
    Err(match response.tag {
        INVALID_REQUEST => Error::InvalidRequest,
        BUFFER_TOO_SMALL => Error::BufferTooSmall,
        UNAVAILABLE => Error::Unavailable,
        BUSY => Error::Busy,
        _ => Error::InvalidRequest,
    })
}

pub fn call(
    channel: &Handle<Channel>,
    message: &mut Message,
    deadline_ns: u64,
) -> Result<ReceivedMessage, Error> {
    if matches!(deadline_ns, 0 | u64::MAX) {
        return Err(Error::InvalidRequest);
    }
    response(
        channel
            .call(message, Some(deadline_ns))
            .map_err(Error::Call)?,
    )
}
