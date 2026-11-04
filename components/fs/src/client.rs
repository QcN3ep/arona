// SPDX-License-Identifier: MPL-2.0

use core::cell::Cell;
use runtime::{Channel, Handle, Rights, Task, ipc::Message};

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
    length: Cell<usize>,
}

impl<'a> Client<'a> {
    pub fn from_channel(task: &'a Handle<Task>, channel: Handle<Channel>) -> Self {
        Self { task, channel }
    }

    pub fn open(&self, path: &str, deadline: u64) -> Result<File<'a>, Error> {
        self.open_bytes(path.as_bytes(), deadline)
    }

    pub fn open_bytes(&self, path: &[u8], deadline: u64) -> Result<File<'a>, Error> {
        self.open_request(protocol::OPEN, path, deadline)
    }

    pub fn create(&self, path: &str, deadline: u64) -> Result<File<'a>, Error> {
        self.open_request(protocol::CREATE, path.as_bytes(), deadline)
    }

    fn open_request(&self, method: u64, path: &[u8], deadline: u64) -> Result<File<'a>, Error> {
        crate::path::components(path)?;
        if path.is_empty() {
            return Err(Error::InvalidRequest);
        }
        let mut path_frames = self.task.allocate(
            path.len().div_ceil(PAGE_SIZE) as u64,
            Rights::READ_WRITE | Rights::TRANSFER,
        )?;
        path_frames.write(0, path)?;
        let (file, server) = self.task.channel(
            CHANNEL_RIGHTS,
            Rights::READ | Rights::WAIT | Rights::TRANSFER,
        )?;
        let mut request = Message::new(method, &[path.len() as u64])?;
        transfer(&mut request, path_frames, Rights::READ)?;
        transfer(&mut request, server, Rights::READ | Rights::WAIT)?;
        let response = call(&self.channel, &mut request, deadline)?;
        let [length] = response.words() else {
            return Err(Error::InvalidData);
        };
        Ok(File {
            task: self.task,
            channel: file,
            length: Cell::new(usize::try_from(*length).map_err(|_| Error::InvalidData)?),
        })
    }
    fn path_request(&self, method: u64, path: &[u8], deadline: u64) -> Result<(), Error> {
        let mut request = Message::new(method, &[path.len() as u64])?;
        transfer(&mut request, self.path(path)?, Rights::READ)?;
        let response = call(&self.channel, &mut request, deadline)?;
        if !response.words().is_empty() {
            return Err(Error::InvalidData);
        }
        Ok(())
    }
    fn path(&self, path: &[u8]) -> Result<Handle<runtime::FrameAllocation>, Error> {
        crate::path::components(path)?;
        let mut frames = self
            .task
            .allocate(1, Rights::READ_WRITE | Rights::TRANSFER)?;
        frames.write(0, path)?;
        Ok(frames)
    }
    pub fn mkdir(&self, path: &str, deadline: u64) -> Result<(), Error> {
        self.path_request(protocol::MKDIR, path.as_bytes(), deadline)
    }
    pub fn unlink(&self, path: &str, deadline: u64) -> Result<(), Error> {
        self.path_request(protocol::UNLINK, path.as_bytes(), deadline)
    }
    pub fn rename(&self, source: &str, target: &str, deadline: u64) -> Result<(), Error> {
        let mut request = Message::new(
            protocol::RENAME,
            &[source.len() as u64, target.len() as u64],
        )?;
        transfer(&mut request, self.path(source.as_bytes())?, Rights::READ)?;
        transfer(&mut request, self.path(target.as_bytes())?, Rights::READ)?;
        let response = call(&self.channel, &mut request, deadline)?;
        if !response.words().is_empty() {
            return Err(Error::InvalidData);
        }
        Ok(())
    }
    pub fn read_dir(
        &self,
        path: &str,
        deadline: u64,
    ) -> Result<alloc::vec::Vec<crate::DirEntry>, Error> {
        let output = self
            .task
            .allocate(1, Rights::READ_WRITE | Rights::DUPLICATE | Rights::TRANSFER)?;
        let mut result = alloc::vec::Vec::new();
        loop {
            let mut request =
                Message::new(protocol::LIST, &[path.len() as u64, result.len() as u64])?;
            transfer(&mut request, self.path(path.as_bytes())?, Rights::READ)?;
            transfer(
                &mut request,
                output.try_clone(Rights::WRITE | Rights::TRANSFER)?,
                Rights::WRITE,
            )?;
            let response = call(&self.channel, &mut request, deadline)?;
            let [count, length] = *response.words() else {
                return Err(Error::InvalidData);
            };
            if length > PAGE_SIZE as u64 || count > length / 3 {
                return Err(Error::InvalidData);
            }
            let mut bytes = [0; PAGE_SIZE];
            output.read(0, &mut bytes[..length as usize])?;
            let mut inside = 0;
            for _ in 0..count {
                if inside + 3 > length as usize || bytes[inside] > 1 {
                    return Err(Error::InvalidData);
                }
                let size = usize::from(u16::from_le_bytes([bytes[inside + 1], bytes[inside + 2]]));
                if size == 0 || size > 255 || inside + 3 + size > length as usize {
                    return Err(Error::InvalidData);
                }
                result.push(crate::DirEntry {
                    name: bytes[inside + 3..inside + 3 + size].to_vec(),
                    directory: bytes[inside] != 0,
                });
                inside += 3 + size;
            }
            if inside != length as usize {
                return Err(Error::InvalidData);
            }
            if count == 0 {
                return Ok(result);
            }
        }
    }
}

impl File<'_> {
    pub fn fsync(&self, deadline: u64) -> Result<(), Error> {
        let response = call(
            &self.channel,
            &mut Message::new(protocol::FSYNC, &[])?,
            deadline,
        )?;
        if !response.words().is_empty() {
            return Err(Error::InvalidData);
        }
        Ok(())
    }

    pub fn write_at(&self, offset: usize, input: &[u8], deadline: u64) -> Result<usize, Error> {
        let length = input.len();
        if length == 0 {
            return Ok(0);
        }
        let mut frames = self.task.allocate(
            length.div_ceil(PAGE_SIZE) as u64,
            Rights::READ_WRITE | Rights::TRANSFER,
        )?;
        frames.write(0, input)?;
        let mut request = Message::new(protocol::WRITE, &[offset as u64, length as u64])?;
        transfer(&mut request, frames, Rights::READ)?;
        let response = call(&self.channel, &mut request, deadline)?;
        let [count, size] = *response.words() else {
            return Err(Error::InvalidData);
        };
        if count > length as u64 {
            return Err(Error::InvalidData);
        }
        self.length
            .set(usize::try_from(size).map_err(|_| Error::InvalidData)?);
        Ok(count as usize)
    }
    pub fn truncate(&self, length: usize, deadline: u64) -> Result<(), Error> {
        let response = call(
            &self.channel,
            &mut Message::new(protocol::TRUNCATE, &[length as u64])?,
            deadline,
        )?;
        if response.words() != [length as u64] {
            return Err(Error::InvalidData);
        }
        self.length.set(length);
        Ok(())
    }
    pub fn size(&self, deadline: u64) -> Result<usize, Error> {
        let response = call(
            &self.channel,
            &mut Message::new(protocol::FILE_INFO, &[])?,
            deadline,
        )?;
        let [size] = *response.words() else {
            return Err(Error::InvalidData);
        };
        let size = usize::try_from(size).map_err(|_| Error::InvalidData)?;
        self.length.set(size);
        Ok(size)
    }

    pub fn read_at(&self, offset: usize, output: &mut [u8], deadline: u64) -> Result<usize, Error> {
        let length = output.len();
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
        self.length.get()
    }

    fn read_at(&self, offset: usize, output: &mut [u8]) -> Result<usize, Error> {
        let deadline = runtime::thread::now_ns()?
            .checked_add(READ_TIMEOUT_NS)
            .ok_or(Error::InvalidRequest)?;
        self.read_at(offset, output, deadline)
    }
}
