// SPDX-License-Identifier: MPL-2.0

use core::fmt;
use devmgr::uart::Session;
use fs::File;
use runtime::{
    Channel, Handle, KernelLog, Rights, Task,
    ipc::{Message, WaitMode},
    thread,
};

use crate::{
    Error, MAX_RECORD_SIZE, Record,
    protocol::{self, CHANNEL_RIGHTS, Submission, call, transfer},
};

pub struct Client<'a> {
    task: &'a Handle<Task>,
    channel: Handle<Channel>,
}

pub struct Control<'a> {
    task: &'a Handle<Task>,
    channel: Handle<Channel>,
}

impl<'a> Client<'a> {
    pub fn from_channel(task: &'a Handle<Task>, channel: Handle<Channel>) -> Result<Self, Error> {
        if !channel.info()?.rights.contains(Rights::READ_WRITE) {
            return Err(Error::InvalidRequest);
        }
        Ok(Self { task, channel })
    }

    pub fn into_channel(self) -> Handle<Channel> {
        self.channel
    }

    pub fn try_clone(&self) -> Result<Self, Error> {
        Ok(Self {
            task: self.task,
            channel: self.channel.try_clone(self.channel.info()?.rights)?,
        })
    }

    /// 仅确认入队，队列满时立即返回错误。
    pub fn try_write(&self, level: log::Level, target: &str, message: &str) -> Result<(), Error> {
        self.try_write_fmt(level, target, format_args!("{message}"))
    }

    pub fn try_write_fmt(
        &self,
        level: log::Level,
        target: &str,
        message: fmt::Arguments<'_>,
    ) -> Result<(), Error> {
        let mut request = Submission::new(level, target, message).encode(self.task)?;
        self.channel.send(&mut request, WaitMode::Poll)?;
        Ok(())
    }

    /// 等待日志进入内存缓冲区并返回序号。
    pub fn write(
        &self,
        level: log::Level,
        target: &str,
        message: &str,
        deadline_ns: u64,
    ) -> Result<u64, Error> {
        self.write_fmt(level, target, format_args!("{message}"), deadline_ns)
    }

    pub fn write_fmt(
        &self,
        level: log::Level,
        target: &str,
        message: fmt::Arguments<'_>,
        deadline_ns: u64,
    ) -> Result<u64, Error> {
        let mut request = Submission::new(level, target, message).encode(self.task)?;
        let reply = call(&self.channel, &mut request, deadline_ns)?;
        match reply.words() {
            [sequence] if *sequence != 0 && *sequence != u64::MAX => Ok(*sequence),
            _ => Err(Error::InvalidRequest),
        }
    }

    /// 完成先前日志的输出并同步日志文件。
    pub fn flush(&self, deadline_ns: u64) -> Result<(), Error> {
        let reply = call(
            &self.channel,
            &mut Message::new(protocol::FLUSH, &[])?,
            deadline_ns,
        )?;
        if reply.words().is_empty() {
            Ok(())
        } else {
            Err(Error::InvalidRequest)
        }
    }
}

impl<'a> Control<'a> {
    pub fn start(
        task: &'a Handle<Task>,
        channel: Handle<Channel>,
        output: Session,
        kernel_log: Handle<KernelLog>,
        persistent_log: Option<File<'a>>,
    ) -> Result<(Self, Client<'a>), Error> {
        let client_rights = CHANNEL_RIGHTS | Rights::DUPLICATE | Rights::TRANSFER;
        let (client, requests) = task.channel(
            client_rights,
            Rights::READ | Rights::WAIT | Rights::TRANSFER,
        )?;
        let mut startup = Message::new(protocol::START, &[])?;
        transfer(&mut startup, output.into_channel(), CHANNEL_RIGHTS)?;
        transfer(&mut startup, kernel_log, Rights::READ)?;
        transfer(&mut startup, requests, Rights::READ | Rights::WAIT)?;
        if let Some(file) = persistent_log {
            transfer(
                &mut startup,
                file.into_channel(),
                Rights::READ_WRITE | Rights::WAIT,
            )?;
        }
        let deadline = thread::now_ns()?
            .checked_add(10_000_000_000)
            .ok_or(Error::Unavailable)?;
        let ready = call(&channel, &mut startup, deadline)?;
        if !ready.words().is_empty() {
            return Err(Error::InvalidRequest);
        }
        Ok((
            Self { task, channel },
            Client {
                task,
                channel: client,
            },
        ))
    }

    pub fn read<'b>(
        &self,
        cursor: u64,
        output: &'b mut [u8],
        deadline_ns: u64,
    ) -> Result<Option<Record<'b>>, Error> {
        self.read_record(protocol::READ, cursor, output, deadline_ns)
    }

    pub fn read_kernel<'b>(
        &self,
        cursor: u64,
        output: &'b mut [u8],
        deadline_ns: u64,
    ) -> Result<Option<Record<'b>>, Error> {
        self.read_record(protocol::READ_KERNEL, cursor, output, deadline_ns)
    }

    /// 输出一条内核记录，返回其序号；没有记录时返回 None。
    pub fn print_kernel(&self, cursor: u64, deadline_ns: u64) -> Result<Option<u64>, Error> {
        let reply = call(
            &self.channel,
            &mut Message::new(protocol::PRINT_KERNEL, &[cursor])?,
            deadline_ns,
        )?;
        match reply.words() {
            [0] => Ok(None),
            [sequence] if *sequence >= cursor && *sequence != u64::MAX => Ok(Some(*sequence)),
            _ => Err(Error::InvalidRequest),
        }
    }

    fn read_record<'b>(
        &self,
        operation: u64,
        cursor: u64,
        output: &'b mut [u8],
        deadline_ns: u64,
    ) -> Result<Option<Record<'b>>, Error> {
        let frames = self
            .task
            .allocate(1, Rights::READ_WRITE | Rights::DUPLICATE | Rights::TRANSFER)?;
        let mut request = Message::new(
            operation,
            &[cursor, output.len().min(MAX_RECORD_SIZE) as u64],
        )?;
        transfer(
            &mut request,
            frames.try_clone(Rights::WRITE | Rights::TRANSFER)?,
            Rights::WRITE,
        )?;
        let reply = call(&self.channel, &mut request, deadline_ns)?;
        let [length] = reply.words() else {
            return Err(Error::InvalidRequest);
        };
        if *length == 0 {
            return Ok(None);
        }
        let length = usize::try_from(*length).map_err(|_| Error::InvalidRequest)?;
        if length > MAX_RECORD_SIZE {
            return Err(Error::InvalidRequest);
        }
        let bytes = output.get_mut(..length).ok_or(Error::InvalidRequest)?;
        frames.read(0, bytes)?;
        let record = Record::decode(bytes).ok_or(Error::InvalidRequest)?;
        if record.sequence < cursor {
            return Err(Error::InvalidRequest);
        }
        Ok(Some(record))
    }
}
