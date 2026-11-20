// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use alloc::{sync::Arc, vec::Vec};
use core::sync::atomic::AtomicUsize;
use runtime::{Channel, FrameAllocation, Handle, Rights, Task, ipc::Message};

use super::request::{attributes_request, metadata_request, output_buffer};
use super::response::{
    complete_append, complete_directory, complete_empty, complete_map_huge, complete_metadata,
    complete_open, complete_read, complete_size, complete_truncate, complete_write,
    directory_entries,
};
use super::{Client, Directory, File, PAGE_SIZE};
use crate::{
    Error, Metadata,
    protocol::{self, CHANNEL_RIGHTS, response_result, transfer},
};

pub type Pending<T, S = ()> = runtime::ipc::PendingCall<T, S, Error>;
pub type PendingOpen<'a> = Pending<File<'a>, (&'a Handle<Task>, Handle<Channel>)>;
pub type PendingDirectory<'a> =
    Pending<Directory<'a>, (&'a Handle<Task>, Handle<Channel>, Arc<()>)>;
pub type PendingRead = Pending<Vec<u8>, (Handle<FrameAllocation>, usize)>;
pub type PendingMetadata = Pending<Metadata, Handle<FrameAllocation>>;
pub type PendingStatistics = Pending<crate::Statistics, Handle<FrameAllocation>>;
pub type PendingHugepageInfo = Pending<usize>;
pub type PendingHugeMap = Pending<Handle<FrameAllocation>>;
pub type PendingMapping = Pending<super::MappingSource, Handle<Channel>>;
pub type PendingWrite = Pending<usize, (Arc<AtomicUsize>, usize)>;
pub type PendingAppend = Pending<(usize, usize), (Arc<AtomicUsize>, usize)>;
pub type PendingSize = Pending<usize, Arc<AtomicUsize>>;
pub type PendingTruncate = Pending<(), (Arc<AtomicUsize>, usize)>;
pub type PendingVolume = Pending<Option<crate::Volume>, Handle<FrameAllocation>>;
pub type PendingDirectoryPage = Pending<Vec<crate::DirEntry>, Handle<FrameAllocation>>;

impl<'a> Client<'a> {
    pub fn start_volume(&self, index: usize) -> Result<PendingVolume, Error> {
        let output = output_buffer(self.task, crate::Volume::MAX_BYTES)?;
        let mut request = Message::new(protocol::VOLUME_INFO, &[index as u64])?;
        transfer(
            &mut request,
            output.try_clone(Rights::WRITE | Rights::TRANSFER)?,
            Rights::WRITE,
        )?;
        Ok(Pending::new(
            self.channel.start_call(&mut request)?,
            output,
            |output, response| {
                let response = response_result(response)?;
                let [length] = *response.words() else {
                    return Err(Error::InvalidData);
                };
                if length == 0 {
                    return Ok(None);
                }
                let length = usize::try_from(length).map_err(|_| Error::InvalidData)?;
                if length >= crate::Volume::MAX_BYTES {
                    return Err(Error::InvalidData);
                }
                let mut bytes = alloc::vec![0; length];
                output.read(0, &mut bytes)?;
                crate::Volume::decode(&bytes).map(Some)
            },
        ))
    }

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
            (self.task, channel, Arc::clone(&self.identity)),
            complete_directory,
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
        empty_call(&self.channel, self.symlink_message(target, path)?)
    }

    pub fn start_read_link(&self, path: &[u8]) -> Result<PendingRead, Error> {
        let request = self.path_message(protocol::READ_LINK, path)?;
        let (mut request, frames) = super::request::output_request(self.task, request, PAGE_SIZE)?;
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
        let (mut request, channel) = self.open_message(
            method,
            path,
            CHANNEL_RIGHTS | Rights::TRANSFER | Rights::DUPLICATE,
        )?;
        Ok((self.channel.start_call(&mut request)?, channel))
    }

    fn start_path(&self, method: u64, path: &[u8]) -> Result<Pending<()>, Error> {
        empty_call(&self.channel, self.path_message(method, path)?)
    }

    fn start_pair(&self, method: u64, source: &[u8], target: &[u8]) -> Result<Pending<()>, Error> {
        empty_call(&self.channel, self.pair_message(method, source, target)?)
    }
}

impl File<'_> {
    pub fn start_read_link(&self) -> Result<PendingRead, Error> {
        let frames = output_buffer(self.task, PAGE_SIZE)?;
        let mut request = Message::new(protocol::READ_LINK, &[])?;
        transfer(
            &mut request,
            frames.try_clone(Rights::WRITE | Rights::TRANSFER)?,
            Rights::WRITE,
        )?;
        Ok(Pending::new(
            self.channel.start_call(&mut request)?,
            (frames, PAGE_SIZE),
            complete_read,
        ))
    }

    pub fn start_statistics(&self) -> Result<PendingStatistics, Error> {
        statistics_call(
            self.task,
            &self.channel,
            Message::new(protocol::STATFS, &[])?,
        )
    }

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
        let (mut request, frames) = self.read_message(offset, length)?;
        Ok(Pending::new(
            self.channel.start_call(&mut request)?,
            (frames, length),
            complete_read,
        ))
    }

    pub fn start_write_at(&self, offset: usize, input: &[u8]) -> Result<PendingWrite, Error> {
        let mut request = self.write_message(protocol::WRITE, offset, input)?;
        Ok(Pending::new(
            self.channel.start_call(&mut request)?,
            (Arc::clone(&self.length), input.len()),
            |(length, limit), response| complete_write((&length, limit), response),
        ))
    }

    pub fn start_append(&self, input: &[u8]) -> Result<PendingAppend, Error> {
        let mut request = self.write_message(protocol::APPEND, 0, input)?;
        Ok(Pending::new(
            self.channel.start_call(&mut request)?,
            (Arc::clone(&self.length), input.len()),
            |(length, limit), response| complete_append((&length, limit), response),
        ))
    }

    pub fn start_truncate(&self, size: usize) -> Result<PendingTruncate, Error> {
        Ok(Pending::new(
            self.channel
                .start_call(&mut Message::new(protocol::TRUNCATE, &[size as u64])?)?,
            (Arc::clone(&self.length), size),
            |(length, size), response| complete_truncate((&length, size), response),
        ))
    }

    pub fn start_size(&self) -> Result<PendingSize, Error> {
        Ok(Pending::new(
            self.channel
                .start_call(&mut Message::new(protocol::FILE_INFO, &[])?)?,
            Arc::clone(&self.length),
            |length, response| complete_size(&length, response),
        ))
    }

    pub fn start_fsync(&self) -> Result<Pending<()>, Error> {
        empty_call(&self.channel, Message::new(protocol::FSYNC, &[])?)
    }

    pub fn start_mapping_source(&self) -> Result<PendingMapping, Error> {
        Ok(Pending::new(
            self.channel
                .start_call(&mut Message::new(protocol::MAPPING_INFO, &[])?)?,
            self.channel.try_clone(CHANNEL_RIGHTS)?,
            |channel, response| {
                let mut response = protocol::response_status(response)?;
                let [granule, device, inode] = *response.words() else {
                    return Err(Error::InvalidData);
                };
                if response.handles().len() != 1 || !matches!(granule, 0 | 2097152) {
                    return Err(Error::InvalidData);
                }
                let state = response
                    .take_handle(0)
                    .unwrap()
                    .downcast()
                    .map_err(|_| Error::InvalidData)?;
                Ok(super::MappingSource {
                    channel,
                    state,
                    granule: granule as usize,
                    device,
                    inode,
                })
            },
        ))
    }

    /// Probes the huge page size exposed by the file without blocking the caller.
    pub fn start_hugepage_info(&self) -> Result<PendingHugepageInfo, Error> {
        Ok(Pending::new(
            self.channel
                .start_call(&mut Message::new(protocol::HUGEPAGE_INFO, &[])?)?,
            (),
            |(), response| {
                let response = response_result(response)?;
                let [size] = *response.words() else {
                    return Err(Error::InvalidData);
                };
                usize::try_from(size).map_err(|_| Error::InvalidData)
            },
        ))
    }

    /// Requests the materialized block containing `offset` for a huge page mapping.
    pub fn start_map_huge(&self, offset: usize) -> Result<PendingHugeMap, Error> {
        let mut request = Message::new(protocol::MAP_HUGEPAGE, &[offset as u64])?;
        Ok(Pending::new(
            self.channel.start_call(&mut request)?,
            (),
            |(), response| complete_map_huge(response),
        ))
    }
}

impl Directory<'_> {
    pub fn start_statistics(&self) -> Result<PendingStatistics, Error> {
        self.file.start_statistics()
    }

    pub fn start_fsync(&self) -> Result<Pending<()>, Error> {
        self.file.start_fsync()
    }

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
        self.read_dir_call(&[cursor as u64])
    }

    pub fn start_read_dir_all(&self, cursor: usize) -> Result<PendingDirectoryPage, Error> {
        self.read_dir_call(&[cursor as u64, 1])
    }

    fn read_dir_call(&self, words: &[u64]) -> Result<PendingDirectoryPage, Error> {
        let output = output_buffer(self.file.task, PAGE_SIZE)?;
        let mut request = Message::new(protocol::LIST, words)?;
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
                if length > PAGE_SIZE as u64 || count > length / crate::DirEntry::HEADER_SIZE as u64
                {
                    return Err(Error::InvalidData);
                }
                let mut bytes = [0; PAGE_SIZE];
                output.read(0, &mut bytes[..length as usize])?;
                directory_entries(&bytes[..length as usize], count as usize)
            },
        ))
    }
}

pub(super) fn empty_call(
    channel: &Handle<Channel>,
    mut request: Message,
) -> Result<Pending<()>, Error> {
    Ok(Pending::new(
        channel.start_call(&mut request)?,
        (),
        |(), response| complete_empty(response),
    ))
}

pub(super) fn statistics_call(
    task: &Handle<Task>,
    channel: &Handle<Channel>,
    mut request: Message,
) -> Result<PendingStatistics, Error> {
    let output = output_buffer(task, crate::Statistics::SIZE)?;
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
            let mut bytes = [0; crate::Statistics::SIZE];
            output.read(0, &mut bytes)?;
            crate::Statistics::decode(&bytes)
        },
    ))
}

pub(super) fn metadata_call(
    task: &Handle<Task>,
    channel: &Handle<Channel>,
    request: Message,
) -> Result<PendingMetadata, Error> {
    let (mut request, output) = metadata_request(task, request)?;
    Ok(Pending::new(
        channel.start_call(&mut request)?,
        output,
        complete_metadata,
    ))
}

fn attributes_call(
    task: &Handle<Task>,
    channel: &Handle<Channel>,
    attributes: crate::Attributes,
) -> Result<Pending<()>, Error> {
    empty_call(channel, attributes_request(task, attributes)?)
}
