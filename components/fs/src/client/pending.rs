// SPDX-License-Identifier: MPL-2.0

use alloc::{sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicUsize, Ordering};
use runtime::{
    Channel, FrameAllocation, Handle, Rights, Task,
    ipc::{Message, ReceivedMessage},
};

use super::{Client, Directory, File, PAGE_SIZE};
use crate::{
    Error, Metadata,
    protocol::{self, CHANNEL_RIGHTS, response_result, transfer},
};

pub type Pending<T, S = ()> = runtime::ipc::PendingCall<T, S, Error>;
pub type PendingOpen<'a> = Pending<File<'a>, (&'a Handle<Task>, Handle<Channel>)>;
pub type PendingDirectory<'a> = Pending<Directory<'a>, (&'a Handle<Task>, Handle<Channel>)>;
pub type PendingRead = Pending<Vec<u8>, (Handle<FrameAllocation>, usize)>;
pub type PendingMetadata = Pending<Metadata, Handle<FrameAllocation>>;
pub type PendingWrite = Pending<usize, (Arc<AtomicUsize>, usize)>;
pub type PendingAppend = Pending<(usize, usize), (Arc<AtomicUsize>, usize)>;
pub type PendingSize = Pending<usize, Arc<AtomicUsize>>;
pub type PendingTruncate = Pending<(), (Arc<AtomicUsize>, usize)>;
pub type PendingDirectoryPage = Pending<Vec<crate::DirEntry>, Handle<FrameAllocation>>;

impl<'a> Client<'a> {
    pub fn start_open(&self, path: &[u8]) -> Result<PendingOpen<'a>, Error> {
        let (call, channel) = self.open_call(protocol::OPEN, path)?;
        Ok(Pending::new(call, (self.task, channel), complete_open))
    }

    pub fn start_create(&self, path: &[u8]) -> Result<PendingOpen<'a>, Error> {
        let (call, channel) = self.open_call(protocol::CREATE, path)?;
        Ok(Pending::new(call, (self.task, channel), complete_open))
    }

    pub fn start_open_directory(&self, path: &[u8]) -> Result<PendingDirectory<'a>, Error> {
        let (call, channel) = self.open_call(protocol::OPEN_DIRECTORY, path)?;
        Ok(Pending::new(
            call,
            (self.task, channel),
            |state, response| {
                Ok(Directory {
                    file: complete_open(state, response)?,
                })
            },
        ))
    }

    pub fn start_metadata(&self, path: &[u8]) -> Result<PendingMetadata, Error> {
        let mut request = Message::new(protocol::METADATA, &[path.len() as u64])?;
        transfer(&mut request, self.path(path)?, Rights::READ)?;
        metadata_call(self.task, &self.channel, request)
    }

    pub fn start_mkdir(&self, path: &[u8]) -> Result<Pending<()>, Error> {
        self.start_path(protocol::MKDIR, path)
    }

    pub fn start_unlink(&self, path: &[u8]) -> Result<Pending<()>, Error> {
        self.start_path(protocol::UNLINK, path)
    }

    pub fn start_rename(&self, source: &[u8], target: &[u8]) -> Result<Pending<()>, Error> {
        self.start_pair(protocol::RENAME, source, target)
    }

    pub fn start_hard_link(&self, source: &[u8], target: &[u8]) -> Result<Pending<()>, Error> {
        self.start_pair(protocol::HARD_LINK, source, target)
    }

    pub fn start_symlink(&self, target: &[u8], path: &[u8]) -> Result<Pending<()>, Error> {
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
        empty_call(&self.channel, request)
    }

    pub fn start_read_link(&self, path: &[u8]) -> Result<PendingRead, Error> {
        let frames = output_buffer(self.task, PAGE_SIZE)?;
        let mut request = Message::new(protocol::READ_LINK, &[path.len() as u64])?;
        transfer(&mut request, self.path(path)?, Rights::READ)?;
        transfer(
            &mut request,
            frames.try_clone(Rights::WRITE | Rights::TRANSFER)?,
            Rights::WRITE,
        )?;
        Ok(Pending::new(
            self.channel.start_call(&mut request)?,
            (frames, PAGE_SIZE - 1),
            complete_read,
        ))
    }

    fn open_call(
        &self,
        method: u64,
        path: &[u8],
    ) -> Result<(Handle<runtime::Call>, Handle<Channel>), Error> {
        let (channel, server) = self.task.channel(
            CHANNEL_RIGHTS | Rights::TRANSFER,
            Rights::READ | Rights::WAIT | Rights::TRANSFER,
        )?;
        let mut request = Message::new(method, &[path.len() as u64])?;
        transfer(&mut request, self.path(path)?, Rights::READ)?;
        transfer(&mut request, server, Rights::READ | Rights::WAIT)?;
        Ok((self.channel.start_call(&mut request)?, channel))
    }

    fn start_path(&self, method: u64, path: &[u8]) -> Result<Pending<()>, Error> {
        let mut request = Message::new(method, &[path.len() as u64])?;
        transfer(&mut request, self.path(path)?, Rights::READ)?;
        empty_call(&self.channel, request)
    }

    fn start_pair(&self, method: u64, source: &[u8], target: &[u8]) -> Result<Pending<()>, Error> {
        let mut request = Message::new(method, &[source.len() as u64, target.len() as u64])?;
        transfer(&mut request, self.path(source)?, Rights::READ)?;
        transfer(&mut request, self.path(target)?, Rights::READ)?;
        empty_call(&self.channel, request)
    }
}

impl File<'_> {
    pub fn start_metadata(&self) -> Result<PendingMetadata, Error> {
        metadata_call(
            self.task,
            &self.channel,
            Message::new(protocol::METADATA, &[])?,
        )
    }

    pub fn start_set_attributes(
        &self,
        attributes: crate::Attributes,
    ) -> Result<Pending<()>, Error> {
        attributes_call(self.task, &self.channel, attributes)
    }

    pub fn start_read_at(&self, offset: usize, length: usize) -> Result<PendingRead, Error> {
        let frames = output_buffer(self.task, length)?;
        let mut request = Message::new(protocol::READ, &[offset as u64, length as u64])?;
        transfer(
            &mut request,
            frames.try_clone(Rights::WRITE | Rights::TRANSFER)?,
            Rights::WRITE,
        )?;
        Ok(Pending::new(
            self.channel.start_call(&mut request)?,
            (frames, length),
            complete_read,
        ))
    }

    pub fn start_write_at(&self, offset: usize, input: &[u8]) -> Result<PendingWrite, Error> {
        let mut request = Message::new(protocol::WRITE, &[offset as u64, input.len() as u64])?;
        transfer(&mut request, input_buffer(self.task, input)?, Rights::READ)?;
        Ok(Pending::new(
            self.channel.start_call(&mut request)?,
            (Arc::clone(&self.length), input.len()),
            |(length, limit), response| {
                let response = response_result(response)?;
                let [count, size] = *response.words() else {
                    return Err(Error::InvalidData);
                };
                if count > limit as u64 {
                    return Err(Error::InvalidData);
                }
                length.store(
                    usize::try_from(size).map_err(|_| Error::InvalidData)?,
                    Ordering::Relaxed,
                );
                Ok(count as usize)
            },
        ))
    }

    pub fn start_append(&self, input: &[u8]) -> Result<PendingAppend, Error> {
        let mut request = Message::new(protocol::APPEND, &[0, input.len() as u64])?;
        transfer(&mut request, input_buffer(self.task, input)?, Rights::READ)?;
        Ok(Pending::new(
            self.channel.start_call(&mut request)?,
            (Arc::clone(&self.length), input.len()),
            |(length, limit), response| {
                let response = response_result(response)?;
                let [count, size, offset] = *response.words() else {
                    return Err(Error::InvalidData);
                };
                if count > limit as u64 || offset.checked_add(count) != Some(size) {
                    return Err(Error::InvalidData);
                }
                length.store(
                    usize::try_from(size).map_err(|_| Error::InvalidData)?,
                    Ordering::Relaxed,
                );
                Ok((offset as usize, count as usize))
            },
        ))
    }

    pub fn start_truncate(&self, size: usize) -> Result<PendingTruncate, Error> {
        Ok(Pending::new(
            self.channel
                .start_call(&mut Message::new(protocol::TRUNCATE, &[size as u64])?)?,
            (Arc::clone(&self.length), size),
            |(length, size), response| {
                let response = response_result(response)?;
                if response.words() != [size as u64] {
                    return Err(Error::InvalidData);
                }
                length.store(size, Ordering::Relaxed);
                Ok(())
            },
        ))
    }

    pub fn start_size(&self) -> Result<PendingSize, Error> {
        Ok(Pending::new(
            self.channel
                .start_call(&mut Message::new(protocol::FILE_INFO, &[])?)?,
            Arc::clone(&self.length),
            |length, response| {
                let size = response_size(response_result(response)?)?;
                length.store(size, Ordering::Relaxed);
                Ok(size)
            },
        ))
    }

    pub fn start_fsync(&self) -> Result<Pending<()>, Error> {
        empty_call(&self.channel, Message::new(protocol::FSYNC, &[])?)
    }
}

impl Directory<'_> {
    pub fn start_metadata(&self) -> Result<PendingMetadata, Error> {
        self.file.start_metadata()
    }

    pub fn start_set_attributes(
        &self,
        attributes: crate::Attributes,
    ) -> Result<Pending<()>, Error> {
        self.file.start_set_attributes(attributes)
    }

    pub fn start_read_dir(&self, cursor: usize) -> Result<PendingDirectoryPage, Error> {
        let output = output_buffer(self.file.task, PAGE_SIZE)?;
        let mut request = Message::new(protocol::LIST, &[cursor as u64])?;
        transfer(
            &mut request,
            output.try_clone(Rights::WRITE | Rights::TRANSFER)?,
            Rights::WRITE,
        )?;
        Ok(Pending::new(
            self.file.channel.start_call(&mut request)?,
            output,
            |output, response| {
                let response = response_result(response)?;
                let [count, length] = *response.words() else {
                    return Err(Error::InvalidData);
                };
                if length > PAGE_SIZE as u64 || count > length / 3 {
                    return Err(Error::InvalidData);
                }
                let mut bytes = [0; PAGE_SIZE];
                output.read(0, &mut bytes[..length as usize])?;
                directory_entries(&bytes[..length as usize], count as usize)
            },
        ))
    }
}

fn complete_open<'a>(
    (task, channel): (&'a Handle<Task>, Handle<Channel>),
    response: ReceivedMessage,
) -> Result<File<'a>, Error> {
    let length = response_size(response_result(response)?)?;
    Ok(File {
        task,
        channel,
        length: Arc::new(AtomicUsize::new(length)),
    })
}

fn response_size(response: ReceivedMessage) -> Result<usize, Error> {
    let [size] = *response.words() else {
        return Err(Error::InvalidData);
    };
    usize::try_from(size).map_err(|_| Error::InvalidData)
}

fn complete_read(
    (frames, limit): (Handle<FrameAllocation>, usize),
    response: ReceivedMessage,
) -> Result<Vec<u8>, Error> {
    let count = response_size(response_result(response)?)?;
    if count > limit {
        return Err(Error::InvalidData);
    }
    let mut bytes = alloc::vec![0; count];
    frames.read(0, &mut bytes)?;
    Ok(bytes)
}

fn empty_call(channel: &Handle<Channel>, mut request: Message) -> Result<Pending<()>, Error> {
    Ok(Pending::new(
        channel.start_call(&mut request)?,
        (),
        |(), response| {
            let response = response_result(response)?;
            if !response.words().is_empty() {
                return Err(Error::InvalidData);
            }
            Ok(())
        },
    ))
}

fn output_buffer(task: &Handle<Task>, length: usize) -> Result<Handle<FrameAllocation>, Error> {
    Ok(task.allocate(
        length.div_ceil(PAGE_SIZE).max(1) as u64,
        Rights::READ_WRITE | Rights::TRANSFER | Rights::DUPLICATE,
    )?)
}

fn input_buffer(task: &Handle<Task>, bytes: &[u8]) -> Result<Handle<FrameAllocation>, Error> {
    let mut frames = task.allocate(
        bytes.len().div_ceil(PAGE_SIZE).max(1) as u64,
        Rights::READ_WRITE | Rights::TRANSFER,
    )?;
    frames.write(0, bytes)?;
    Ok(frames)
}

fn metadata_call(
    task: &Handle<Task>,
    channel: &Handle<Channel>,
    mut request: Message,
) -> Result<PendingMetadata, Error> {
    let output = output_buffer(task, PAGE_SIZE)?;
    transfer(
        &mut request,
        output.try_clone(Rights::WRITE | Rights::TRANSFER)?,
        Rights::WRITE,
    )?;
    Ok(Pending::new(
        channel.start_call(&mut request)?,
        output,
        |output, response| {
            let response = response_result(response)?;
            if !response.words().is_empty() {
                return Err(Error::InvalidData);
            }
            let mut bytes = [0; Metadata::SIZE];
            output.read(0, &mut bytes)?;
            Metadata::decode(&bytes).map_err(|_| Error::InvalidData)
        },
    ))
}

fn attributes_call(
    task: &Handle<Task>,
    channel: &Handle<Channel>,
    attributes: crate::Attributes,
) -> Result<Pending<()>, Error> {
    let mut request = Message::new(protocol::SET_ATTRIBUTES, &[])?;
    transfer(
        &mut request,
        input_buffer(task, &attributes.encode())?,
        Rights::READ,
    )?;
    empty_call(channel, request)
}

pub(super) fn directory_entries(bytes: &[u8], count: usize) -> Result<Vec<crate::DirEntry>, Error> {
    let mut entries = Vec::new();
    let mut inside = 0;
    for _ in 0..count {
        if inside + 3 > bytes.len() || bytes[inside] > 1 {
            return Err(Error::InvalidData);
        }
        let size = usize::from(u16::from_le_bytes([bytes[inside + 1], bytes[inside + 2]]));
        if size == 0 || size > 255 || inside + 3 + size > bytes.len() {
            return Err(Error::InvalidData);
        }
        entries.push(crate::DirEntry {
            name: bytes[inside + 3..inside + 3 + size].to_vec(),
            directory: bytes[inside] != 0,
        });
        inside += 3 + size;
    }
    if inside != bytes.len() {
        return Err(Error::InvalidData);
    }
    Ok(entries)
}
