// SPDX-License-Identifier: MPL-2.0

use runtime::{Channel, FrameAllocation, Handle, Rights, Task, ipc::Message};

use crate::{
    Error,
    protocol::{self, CHANNEL_RIGHTS, call, transfer},
};

const PAGE_SIZE: usize = 4096;
const READ_TIMEOUT_NS: u64 = 10_000_000_000;

pub struct Client<'a> {
    task: &'a Handle<Task>,
    channel: Handle<Channel>,
}

pub struct File<'a> {
    task: &'a Handle<Task>,
    channel: Handle<Channel>,
    length: usize,
}

impl<'a> Client<'a> {
    pub fn start(
        task: &'a Handle<Task>,
        channel: Handle<Channel>,
        frames: Handle<FrameAllocation>,
        length: usize,
        deadline: u64,
    ) -> Result<Self, Error> {
        let mut request = Message::new(protocol::START, &[length as u64])?;
        transfer(&mut request, frames, Rights::READ)?;
        let response = call(&channel, &mut request, deadline)?;
        if !response.words().is_empty() {
            return Err(Error::InvalidData);
        }
        Ok(Self { task, channel })
    }

    pub fn open(&self, path: &str, deadline: u64) -> Result<File<'a>, Error> {
        if path.is_empty() || path.as_bytes().contains(&0) {
            return Err(Error::InvalidRequest);
        }
        let mut path_frames = self.task.allocate(
            path.len().div_ceil(PAGE_SIZE) as u64,
            Rights::READ_WRITE | Rights::TRANSFER,
        )?;
        path_frames.write(0, path.as_bytes())?;
        let (file, server) = self.task.channel(
            CHANNEL_RIGHTS,
            Rights::READ | Rights::WAIT | Rights::TRANSFER,
        )?;
        let mut request = Message::new(protocol::OPEN, &[path.len() as u64])?;
        transfer(&mut request, path_frames, Rights::READ)?;
        transfer(&mut request, server, Rights::READ | Rights::WAIT)?;
        let response = call(&self.channel, &mut request, deadline)?;
        let [length] = response.words() else {
            return Err(Error::InvalidData);
        };
        Ok(File {
            task: self.task,
            channel: file,
            length: usize::try_from(*length).map_err(|_| Error::InvalidData)?,
        })
    }
}

impl File<'_> {
    pub fn read_at(&self, offset: usize, output: &mut [u8], deadline: u64) -> Result<usize, Error> {
        let length = output.len().min(self.length.saturating_sub(offset));
        if length == 0 {
            return Ok(0);
        }
        let frames = self.task.allocate(
            length.div_ceil(PAGE_SIZE) as u64,
            Rights::READ_WRITE | Rights::DUPLICATE | Rights::TRANSFER,
        )?;
        let mut request = Message::new(protocol::READ, &[offset as u64, length as u64])?;
        transfer(
            &mut request,
            frames.try_clone(Rights::WRITE | Rights::TRANSFER)?,
            Rights::WRITE,
        )?;
        let response = call(&self.channel, &mut request, deadline)?;
        let [count] = response.words() else {
            return Err(Error::InvalidData);
        };
        let count = usize::try_from(*count).map_err(|_| Error::InvalidData)?;
        if count > length {
            return Err(Error::InvalidData);
        }
        frames.read(0, &mut output[..count])?;
        Ok(count)
    }
}

impl crate::ReadAt for File<'_> {
    fn len(&self) -> usize {
        self.length
    }

    fn read_at(&self, offset: usize, output: &mut [u8]) -> Result<usize, Error> {
        let deadline = runtime::thread::now_ns()?
            .checked_add(READ_TIMEOUT_NS)
            .ok_or(Error::InvalidRequest)?;
        self.read_at(offset, output, deadline)
    }
}
