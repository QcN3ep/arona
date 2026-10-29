// SPDX-License-Identifier: MPL-2.0

//! 日志记录的保存、管理请求与 UART 输出。

use alloc::{collections::VecDeque, format, vec, vec::Vec};
use core::fmt::Write;
use devmgr::uart::Session;
use runtime::{
    Channel, FrameAllocation, Handle, KernelError, KernelLog, Reply,
    ipc::{self, Message, ReceivedMessage, WaitItem, WaitMode},
    logger::MAX_RECORD_SIZE,
    thread,
};

use crate::{
    Error, Record,
    protocol::{self, Submission},
};

const RETAINED_BYTES: usize = 1024 * 1024;

struct Service {
    output: Option<Session>,
    kernel_log: Handle<KernelLog>,
    records: VecDeque<Vec<u8>>,
    retained: usize,
    sequence: u64,
}

pub fn run(control: Handle<Channel>) -> Result<(), Error> {
    let mut startup = control.receive(WaitMode::Forever)?;
    let reply = startup.take_reply().ok_or(Error::InvalidRequest)?;
    if startup.tag != protocol::START || !startup.words().is_empty() || startup.handles().len() != 3
    {
        return Err(Error::InvalidRequest);
    }
    let output = Session::from_channel(
        startup
            .take_handle(0)
            .unwrap()
            .downcast()
            .map_err(|_| Error::InvalidRequest)?,
    )?;
    let kernel_log = startup
        .take_handle(0)
        .unwrap()
        .downcast()
        .map_err(|_| Error::InvalidRequest)?;
    let requests = startup
        .take_handle(0)
        .unwrap()
        .downcast::<Channel>()
        .map_err(|_| Error::InvalidRequest)?;
    let mut service = Service {
        output: Some(output),
        kernel_log,
        records: VecDeque::new(),
        retained: 0,
        sequence: 0,
    };
    deliver(&reply, Ok(Message::new(protocol::SUCCESS, &[])?))?;
    let mut requests = Some(requests);
    loop {
        match control.receive(WaitMode::Poll) {
            Ok(message) => service.dispatch(message, true)?,
            Err(runtime::Error::Kernel(KernelError::WouldBlock)) => {}
            Err(runtime::Error::Kernel(KernelError::EndOfStream)) => return Ok(()),
            Err(error) => return Err(error.into()),
        }
        if let Some(channel) = &requests {
            match channel.receive(WaitMode::Poll) {
                Ok(message) => {
                    service.dispatch(message, false)?;
                    continue;
                }
                Err(runtime::Error::Kernel(KernelError::WouldBlock)) => {}
                Err(runtime::Error::Kernel(KernelError::EndOfStream)) => {
                    requests = None;
                }
                Err(error) => return Err(error.into()),
            }
        }
        let mut items = core::array::from_fn::<_, 2, _>(|_| {
            WaitItem::new(&control, ipc::READABLE | ipc::PEER_CLOSED)
        });
        let count = if let Some(requests) = &requests {
            items[1] = WaitItem::new(requests, ipc::READABLE | ipc::PEER_CLOSED);
            2
        } else {
            1
        };
        ipc::wait_many(&items[..count], WaitMode::Forever)?;
    }
}

impl Service {
    fn dispatch(&mut self, mut message: ReceivedMessage, control: bool) -> Result<(), Error> {
        let reply = message.take_reply();
        if !control && message.tag == protocol::WRITE {
            let result =
                Submission::decode(message).and_then(|submission| self.publish(submission));
            match result {
                Ok(bytes) => {
                    let record = Record::decode(&bytes).ok_or(Error::InvalidRequest)?;
                    if let Some(reply) = &reply {
                        deliver(
                            reply,
                            Ok(Message::new(protocol::SUCCESS, &[record.sequence])?),
                        )?;
                    }
                    let _ = self.output(record, "user");
                }
                Err(error) => {
                    if let Some(reply) = &reply {
                        deliver(reply, Err(error))?;
                    }
                }
            }
        } else if let Some(reply) = reply {
            let result = if control {
                self.manage(message)
            } else {
                self.flush(message)
            };
            deliver(&reply, result)?;
        }
        Ok(())
    }

    fn publish(&mut self, submission: Submission) -> Result<Vec<u8>, Error> {
        let sequence = self
            .sequence
            .checked_add(1)
            .filter(|sequence| *sequence < u64::MAX)
            .ok_or(Error::Unavailable)?;
        let (target, message) = submission.text.split_at(submission.target_length);
        let record = Record {
            sequence,
            timestamp_ns: Some(thread::now_ns()?),
            level: submission.level,
            truncated: submission.truncated,
            target,
            message,
        };
        let mut bytes = vec![0; abi::logger::HEADER_SIZE + submission.text.len()];
        let length = record.encode(&mut bytes).ok_or(Error::InvalidRequest)?;
        while self.retained + length > RETAINED_BYTES {
            self.retained -= self.records.pop_front().unwrap().len();
        }
        self.retained += length;
        self.records.push_back(bytes.clone());
        self.sequence = sequence;
        Ok(bytes)
    }

    fn manage(&mut self, mut message: ReceivedMessage) -> Result<Message, Error> {
        match (message.tag, message.words()) {
            (protocol::READ | protocol::READ_KERNEL, [cursor, capacity])
                if message.handles().len() == 1 =>
            {
                let cursor = *cursor;
                let capacity = *capacity;
                let mut bytes = [0; MAX_RECORD_SIZE];
                let length = if message.tag == protocol::READ_KERNEL {
                    self.kernel_log
                        .read(cursor, &mut bytes)?
                        .map_or(0, |record| {
                            abi::logger::HEADER_SIZE + record.target.len() + record.message.len()
                        })
                } else {
                    let record = self
                        .records
                        .iter()
                        .find(|bytes| Record::decode(bytes).unwrap().sequence >= cursor);
                    if let Some(record) = record {
                        bytes[..record.len()].copy_from_slice(record);
                        record.len()
                    } else {
                        0
                    }
                };
                if capacity < length as u64 {
                    return Err(Error::BufferTooSmall);
                }
                let mut output = message
                    .take_handle(0)
                    .unwrap()
                    .downcast::<FrameAllocation>()
                    .map_err(|_| Error::InvalidRequest)?;
                if length != 0 {
                    output.write(0, &bytes[..length])?;
                }
                Ok(Message::new(protocol::SUCCESS, &[length as u64])?)
            }
            (protocol::PRINT_KERNEL, [cursor]) if message.handles().is_empty() => {
                let mut bytes = [0; MAX_RECORD_SIZE];
                let sequence = if let Some(record) = self.kernel_log.read(*cursor, &mut bytes)? {
                    self.output(record, "kernel")?;
                    record.sequence
                } else {
                    0
                };
                Ok(Message::new(protocol::SUCCESS, &[sequence])?)
            }
            _ => Err(Error::InvalidRequest),
        }
    }

    fn flush(&self, message: ReceivedMessage) -> Result<Message, Error> {
        if message.tag != protocol::FLUSH
            || !message.words().is_empty()
            || !message.handles().is_empty()
        {
            return Err(Error::InvalidRequest);
        }
        self.output.as_ref().ok_or(Error::Unavailable)?.flush()?;
        Ok(Message::new(protocol::SUCCESS, &[])?)
    }

    fn output(&mut self, record: Record<'_>, source: &str) -> Result<(), Error> {
        let mut line = match record.timestamp_ns {
            Some(time) => format!(
                "[{:>5}.{:06}] ",
                time / 1_000_000_000,
                time % 1_000_000_000 / 1000
            ),
            None => "[    early   ] ".into(),
        };
        let level = match record.level {
            crate::Level::Error => "ERROR",
            crate::Level::Warn => "WARN",
            crate::Level::Info => "INFO",
            crate::Level::Debug => "DEBUG",
            crate::Level::Trace => "TRACE",
        };
        write!(
            line,
            "{level} {source}/{}: {}",
            record.target.escape_debug(),
            record.message.escape_debug()
        )
        .unwrap();
        if record.truncated {
            line.push_str(" [truncated]");
        }
        line.push('\n');
        if let Err(error) = self
            .output
            .as_ref()
            .ok_or(Error::Unavailable)?
            .write_all(line.as_bytes())
        {
            self.output = None;
            return Err(error.into());
        }
        Ok(())
    }
}

fn deliver(reply: &Handle<Reply>, result: Result<Message, Error>) -> Result<(), Error> {
    let mut message = match result {
        Ok(message) => message,
        Err(error) => Message::new(
            match error {
                Error::InvalidRequest => protocol::INVALID_REQUEST,
                Error::BufferTooSmall => protocol::BUFFER_TOO_SMALL,
                Error::Busy | Error::Runtime(runtime::Error::Kernel(KernelError::Busy)) => {
                    protocol::BUSY
                }
                _ => protocol::UNAVAILABLE,
            },
            &[],
        )?,
    };
    match reply.send(&mut message) {
        Ok(()) | Err(runtime::Error::Kernel(KernelError::InvalidState)) => Ok(()),
        Err(error) => Err(error.into()),
    }
}
