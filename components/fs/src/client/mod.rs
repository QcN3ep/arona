// SPDX-License-Identifier: MPL-2.0

mod pending;

pub use pending::{
    Pending, PendingAppend, PendingDirectory, PendingDirectoryPage, PendingMetadata, PendingOpen,
    PendingRead, PendingSize, PendingTruncate, PendingWrite,
};

use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};
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
    length: Arc<AtomicUsize>,
}

pub struct Directory<'a> {
    file: File<'a>,
}

impl<'a> Client<'a> {
    pub fn from_channel(task: &'a Handle<Task>, channel: Handle<Channel>) -> Self {
        Self { task, channel }
    }

    pub fn channel(&self) -> &Handle<Channel> {
        &self.channel
    }

    pub fn into_channel(self) -> Handle<Channel> {
        self.channel
    }

    pub fn open(&self, path: &str, deadline: u64) -> Result<File<'a>, Error> {
        self.open_bytes(path.as_bytes(), deadline)
    }

    pub fn open_bytes(&self, path: &[u8], deadline: u64) -> Result<File<'a>, Error> {
        self.open_request(protocol::OPEN, path, deadline)
    }

    pub fn create(&self, path: &str, deadline: u64) -> Result<File<'a>, Error> {
        self.create_bytes(path.as_bytes(), deadline)
    }

    pub fn create_bytes(&self, path: &[u8], deadline: u64) -> Result<File<'a>, Error> {
        self.open_request(protocol::CREATE, path, deadline)
    }

    pub fn open_directory(&self, path: &[u8], deadline: u64) -> Result<Directory<'a>, Error> {
        Ok(Directory {
            file: self.open_request(protocol::OPEN_DIRECTORY, path, deadline)?,
        })
    }

    pub fn metadata(&self, path: &[u8], deadline: u64) -> Result<crate::Metadata, Error> {
        let output = self
            .task
            .allocate(1, Rights::READ_WRITE | Rights::TRANSFER | Rights::DUPLICATE)?;
        let mut request = Message::new(protocol::METADATA, &[path.len() as u64])?;
        transfer(&mut request, self.path(path)?, Rights::READ)?;
        transfer(
            &mut request,
            output.try_clone(Rights::WRITE | Rights::TRANSFER)?,
            Rights::WRITE,
        )?;
        let response = call(&self.channel, &mut request, deadline)?;
        if !response.words().is_empty() {
            return Err(Error::InvalidData);
        }
        let mut bytes = [0; crate::Metadata::SIZE];
        output.read(0, &mut bytes)?;
        crate::Metadata::decode(&bytes).map_err(|_| Error::InvalidData)
    }

    pub fn hard_link(&self, source: &[u8], target: &[u8], deadline: u64) -> Result<(), Error> {
        self.pair_request(protocol::HARD_LINK, source, target, deadline)
    }

    pub fn symlink(&self, target: &[u8], path: &[u8], deadline: u64) -> Result<(), Error> {
        if target.is_empty() || target.len() >= PAGE_SIZE || target.contains(&0) {
            return Err(Error::InvalidRequest);
        }
        let mut frames = self
            .task
            .allocate(1, Rights::READ_WRITE | Rights::TRANSFER)?;
        frames.write(0, target)?;
        let mut request =
            Message::new(protocol::SYMLINK, &[target.len() as u64, path.len() as u64])?;
        transfer(&mut request, frames, Rights::READ)?;
        transfer(&mut request, self.path(path)?, Rights::READ)?;
        let response = call(&self.channel, &mut request, deadline)?;
        if !response.words().is_empty() {
            return Err(Error::InvalidData);
        }
        Ok(())
    }

    pub fn read_link(&self, path: &[u8], deadline: u64) -> Result<alloc::vec::Vec<u8>, Error> {
        let output = self
            .task
            .allocate(1, Rights::READ_WRITE | Rights::TRANSFER | Rights::DUPLICATE)?;
        let mut request = Message::new(protocol::READ_LINK, &[path.len() as u64])?;
        transfer(&mut request, self.path(path)?, Rights::READ)?;
        transfer(
            &mut request,
            output.try_clone(Rights::WRITE | Rights::TRANSFER)?,
            Rights::WRITE,
        )?;
        let response = call(&self.channel, &mut request, deadline)?;
        let [length] = *response.words() else {
            return Err(Error::InvalidData);
        };
        if length >= PAGE_SIZE as u64 {
            return Err(Error::InvalidData);
        }
        let mut bytes = alloc::vec![0; length as usize];
        output.read(0, &mut bytes)?;
        Ok(bytes)
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
            CHANNEL_RIGHTS.union(Rights::TRANSFER),
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
            length: Arc::new(AtomicUsize::new(
                usize::try_from(*length).map_err(|_| Error::InvalidData)?,
            )),
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
        self.mkdir_bytes(path.as_bytes(), deadline)
    }
    pub fn unlink(&self, path: &str, deadline: u64) -> Result<(), Error> {
        self.unlink_bytes(path.as_bytes(), deadline)
    }
    pub fn mkdir_bytes(&self, path: &[u8], deadline: u64) -> Result<(), Error> {
        self.path_request(protocol::MKDIR, path, deadline)
    }
    pub fn unlink_bytes(&self, path: &[u8], deadline: u64) -> Result<(), Error> {
        self.path_request(protocol::UNLINK, path, deadline)
    }
    pub fn rename(&self, source: &str, target: &str, deadline: u64) -> Result<(), Error> {
        self.rename_bytes(source.as_bytes(), target.as_bytes(), deadline)
    }
    pub fn rename_bytes(&self, source: &[u8], target: &[u8], deadline: u64) -> Result<(), Error> {
        self.pair_request(protocol::RENAME, source, target, deadline)
    }
    fn pair_request(
        &self,
        method: u64,
        source: &[u8],
        target: &[u8],
        deadline: u64,
    ) -> Result<(), Error> {
        let mut request = Message::new(method, &[source.len() as u64, target.len() as u64])?;
        transfer(&mut request, self.path(source)?, Rights::READ)?;
        transfer(&mut request, self.path(target)?, Rights::READ)?;
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
        self.read_dir_bytes(path.as_bytes(), deadline)
    }
    pub fn read_dir_bytes(
        &self,
        path: &[u8],
        deadline: u64,
    ) -> Result<alloc::vec::Vec<crate::DirEntry>, Error> {
        self.open_directory(path, deadline)?.read_dir(deadline)
    }
}

impl File<'_> {
    pub fn channel(&self) -> &Handle<Channel> {
        &self.channel
    }

    pub fn metadata(&self, deadline: u64) -> Result<crate::Metadata, Error> {
        let output = self
            .task
            .allocate(1, Rights::READ_WRITE | Rights::TRANSFER | Rights::DUPLICATE)?;
        let mut request = Message::new(protocol::METADATA, &[])?;
        transfer(
            &mut request,
            output.try_clone(Rights::WRITE | Rights::TRANSFER)?,
            Rights::WRITE,
        )?;
        let response = call(&self.channel, &mut request, deadline)?;
        if !response.words().is_empty() {
            return Err(Error::InvalidData);
        }
        let mut bytes = [0; crate::Metadata::SIZE];
        output.read(0, &mut bytes)?;
        crate::Metadata::decode(&bytes).map_err(|_| Error::InvalidData)
    }

    pub fn set_attributes(
        &self,
        attributes: crate::Attributes,
        deadline: u64,
    ) -> Result<(), Error> {
        let mut input = self
            .task
            .allocate(1, Rights::READ_WRITE | Rights::TRANSFER)?;
        input.write(0, &attributes.encode())?;
        let mut request = Message::new(protocol::SET_ATTRIBUTES, &[])?;
        transfer(&mut request, input, Rights::READ)?;
        let response = call(&self.channel, &mut request, deadline)?;
        if !response.words().is_empty() {
            return Err(Error::InvalidData);
        }
        Ok(())
    }

    pub fn append(&self, input: &[u8], deadline: u64) -> Result<(usize, usize), Error> {
        if input.is_empty() {
            return Ok((self.size(deadline)?, 0));
        }
        let mut frames = self.task.allocate(
            input.len().div_ceil(PAGE_SIZE) as u64,
            Rights::READ_WRITE | Rights::TRANSFER,
        )?;
        frames.write(0, input)?;
        let mut request = Message::new(protocol::APPEND, &[0, input.len() as u64])?;
        transfer(&mut request, frames, Rights::READ)?;
        let response = call(&self.channel, &mut request, deadline)?;
        let [count, size, offset] = *response.words() else {
            return Err(Error::InvalidData);
        };
        if count > input.len() as u64 || offset.checked_add(count) != Some(size) {
            return Err(Error::InvalidData);
        }
        self.length.store(
            usize::try_from(size).map_err(|_| Error::InvalidData)?,
            Ordering::Relaxed,
        );
        Ok((offset as usize, count as usize))
    }

    pub fn from_channel(
        task: &Handle<Task>,
        channel: Handle<Channel>,
        deadline: u64,
    ) -> Result<File<'_>, Error> {
        if !channel.info()?.rights.contains(CHANNEL_RIGHTS) {
            return Err(Error::InvalidRequest);
        }
        let file = File {
            task,
            channel,
            length: Arc::new(AtomicUsize::new(0)),
        };
        file.size(deadline)?;
        Ok(file)
    }

    pub fn into_channel(self) -> Handle<Channel> {
        self.channel
    }

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
        self.length.store(
            usize::try_from(size).map_err(|_| Error::InvalidData)?,
            Ordering::Relaxed,
        );
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
        self.length.store(length, Ordering::Relaxed);
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
        self.length.store(size, Ordering::Relaxed);
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
        self.length.load(Ordering::Relaxed)
    }

    fn read_at(&self, offset: usize, output: &mut [u8]) -> Result<usize, Error> {
        let deadline = runtime::thread::now_ns()?
            .checked_add(READ_TIMEOUT_NS)
            .ok_or(Error::InvalidRequest)?;
        self.read_at(offset, output, deadline)
    }
}

impl Directory<'_> {
    pub fn channel(&self) -> &Handle<Channel> {
        self.file.channel()
    }

    pub fn metadata(&self, deadline: u64) -> Result<crate::Metadata, Error> {
        self.file.metadata(deadline)
    }
    pub fn set_attributes(
        &self,
        attributes: crate::Attributes,
        deadline: u64,
    ) -> Result<(), Error> {
        self.file.set_attributes(attributes, deadline)
    }
    pub fn read_dir(&self, deadline: u64) -> Result<alloc::vec::Vec<crate::DirEntry>, Error> {
        let output = self
            .file
            .task
            .allocate(1, Rights::READ_WRITE | Rights::DUPLICATE | Rights::TRANSFER)?;
        let mut result = alloc::vec::Vec::new();
        loop {
            let mut request = Message::new(protocol::LIST, &[result.len() as u64])?;
            transfer(
                &mut request,
                output.try_clone(Rights::WRITE | Rights::TRANSFER)?,
                Rights::WRITE,
            )?;
            let response = call(&self.file.channel, &mut request, deadline)?;
            let [count, length] = *response.words() else {
                return Err(Error::InvalidData);
            };
            if length > PAGE_SIZE as u64 || count > length / 3 {
                return Err(Error::InvalidData);
            }
            let mut bytes = [0; PAGE_SIZE];
            output.read(0, &mut bytes[..length as usize])?;
            result.extend(pending::directory_entries(
                &bytes[..length as usize],
                count as usize,
            )?);
            if count == 0 {
                return Ok(result);
            }
        }
    }
}
