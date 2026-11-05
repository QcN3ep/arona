// SPDX-License-Identifier: MPL-2.0

//! 日志缓冲、持久化与 UART 输出。

use alloc::{collections::VecDeque, format, vec, vec::Vec};
use core::fmt::Write;
use devmgr::uart::Session;
use fs::{File, ReadAt};
use runtime::{
    Channel, FrameAllocation, Handle, KernelError, KernelLog, Reply, Task,
    ipc::{self, Message, ReceivedMessage, WaitItem, WaitMode},
    logger::MAX_RECORD_SIZE,
    thread,
};

use crate::{
    Error, Record,
    protocol::{self, Submission},
};

const RETAINED_BYTES: usize = 1024 * 1024;
const IO_TIMEOUT_NS: u64 = 10_000_000_000;

enum Persistence<'a> {
    Disabled,
    Active { file: File<'a>, offset: usize },
    Failed,
}

struct Restored {
    records: VecDeque<Vec<u8>>,
    retained: usize,
    sequence: u64,
    offset: usize,
}

struct Service<'a> {
    output: Option<Session>,
    kernel_log: Handle<KernelLog>,
    persistent: Persistence<'a>,
    records: VecDeque<Vec<u8>>,
    retained: usize,
    sequence: u64,
}

pub fn run(task: &Handle<Task>, control: Handle<Channel>) -> Result<(), Error> {
    let mut startup = control.receive(WaitMode::Forever)?;
    let reply = startup.take_reply().ok_or(Error::InvalidRequest)?;
    if startup.tag != protocol::START
        || !startup.words().is_empty()
        || !(3..=4).contains(&startup.handles().len())
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
    let persistent = match startup.take_handle(0) {
        None => Persistence::Disabled,
        Some(handle) => {
            let channel = handle.downcast().map_err(|_| Error::InvalidRequest)?;
            match deadline()
                .and_then(|deadline| File::from_channel(task, channel, deadline).map_err(|_| ()))
            {
                Ok(file) => Persistence::Active { file, offset: 0 },
                Err(_) => Persistence::Failed,
            }
        }
    };
    let mut service = Service {
        output: Some(output),
        kernel_log,
        persistent,
        records: VecDeque::new(),
        retained: 0,
        sequence: 0,
    };
    service.restore();
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

impl Service<'_> {
    fn restore(&mut self) {
        let Persistence::Active { file, offset } = &mut self.persistent else {
            return;
        };
        match restore(file) {
            Ok(restored) => {
                self.records = restored.records;
                self.retained = restored.retained;
                self.sequence = restored.sequence;
                *offset = restored.offset;
            }
            Err(()) => self.persistent = Persistence::Failed,
        }
    }

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
                    self.persist(&bytes);
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
        record.encode(&mut bytes).ok_or(Error::InvalidRequest)?;
        retain(&mut self.records, &mut self.retained, bytes.clone());
        self.sequence = sequence;
        Ok(bytes)
    }

    fn persist(&mut self, bytes: &[u8]) {
        let Persistence::Active { file, offset } = &mut self.persistent else {
            return;
        };
        let result = offset.checked_add(bytes.len()).ok_or(()).and_then(|end| {
            write_all_at(file, *offset, bytes)?;
            *offset = end;
            Ok(())
        });
        if result.is_err() {
            self.persistent = Persistence::Failed;
        }
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

    fn flush(&mut self, message: ReceivedMessage) -> Result<Message, Error> {
        if message.tag != protocol::FLUSH
            || !message.words().is_empty()
            || !message.handles().is_empty()
        {
            return Err(Error::InvalidRequest);
        }
        let mut failed = false;
        if self
            .output
            .as_ref()
            .is_none_or(|output| output.flush().is_err())
        {
            self.output = None;
            failed = true;
        }
        if let Persistence::Active { file, .. } = &self.persistent
            && deadline()
                .and_then(|deadline| file.fsync(deadline).map_err(|_| ()))
                .is_err()
        {
            self.persistent = Persistence::Failed;
        }
        if failed || matches!(self.persistent, Persistence::Failed) {
            return Err(Error::Unavailable);
        }
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

fn restore(file: &File<'_>) -> Result<Restored, ()> {
    let size = file.len();
    let mut input = vec![0; 4096 + MAX_RECORD_SIZE];
    let mut buffered = 0;
    let mut records = VecDeque::new();
    let mut retained = 0;
    let mut sequence = 0;
    let mut offset = 0;
    while offset < size {
        if buffered < MAX_RECORD_SIZE && offset + buffered < size {
            let count = file
                .read_at(offset + buffered, &mut input[buffered..], deadline()?)
                .map_err(|_| ())?;
            if count == 0 {
                return Err(());
            }
            buffered += count;
            continue;
        }
        if buffered < abi::logger::HEADER_SIZE {
            break;
        }
        let target = u32::from_le_bytes(input[24..28].try_into().unwrap()) as usize;
        let message = u32::from_le_bytes(input[28..32].try_into().unwrap()) as usize;
        let Some(length) = abi::logger::HEADER_SIZE
            .checked_add(target)
            .and_then(|length| length.checked_add(message))
            .filter(|length| *length <= MAX_RECORD_SIZE && *length <= buffered)
        else {
            break;
        };
        let Some(record) = Record::decode(&input[..length]) else {
            break;
        };
        if record.sequence <= sequence {
            break;
        }
        sequence = record.sequence;
        retain(&mut records, &mut retained, input[..length].to_vec());
        offset += length;
        input.copy_within(length..buffered, 0);
        buffered -= length;
    }
    if offset != size {
        file.truncate(offset, deadline()?).map_err(|_| ())?;
        file.fsync(deadline()?).map_err(|_| ())?;
    }
    Ok(Restored {
        records,
        retained,
        sequence,
        offset,
    })
}

fn retain(records: &mut VecDeque<Vec<u8>>, retained: &mut usize, bytes: Vec<u8>) {
    while *retained + bytes.len() > RETAINED_BYTES {
        *retained -= records.pop_front().unwrap().len();
    }
    *retained += bytes.len();
    records.push_back(bytes);
}

fn write_all_at(file: &File<'_>, mut offset: usize, mut input: &[u8]) -> Result<(), ()> {
    while !input.is_empty() {
        let count = file.write_at(offset, input, deadline()?).map_err(|_| ())?;
        if count == 0 {
            return Err(());
        }
        offset = offset.checked_add(count).ok_or(())?;
        input = &input[count..];
    }
    Ok(())
}

fn deadline() -> Result<u64, ()> {
    thread::now_ns()
        .map_err(|_| ())?
        .checked_add(IO_TIMEOUT_NS)
        .ok_or(())
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
