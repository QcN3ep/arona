// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

mod path;
mod pending;
mod request;
mod response;

pub use path::{OpenedNode, PendingNode};

pub use pending::{
    Pending, PendingAppend, PendingDirectory, PendingDirectoryPage, PendingHugeMap,
    PendingHugepageInfo, PendingMapping, PendingMetadata, PendingOpen, PendingRead, PendingSize,
    PendingTruncate, PendingWrite,
};
pub use pending::{PendingStatistics, PendingVolume};

use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};
use runtime::{
    Channel, FrameAllocation, Handle, Rights, Task,
    ipc::{Message, WaitMode},
};

use crate::{
    Error,
    protocol::{self, CHANNEL_RIGHTS, call, transfer},
};

const PAGE_SIZE: usize = 4096;
const READ_TIMEOUT_NS: u64 = 10_000_000_000;

pub struct Client<'a> {
    task: &'a Handle<Task>,
    channel: Handle<Channel>,
    identity: Arc<()>,
}

pub struct File<'a> {
    task: &'a Handle<Task>,
    channel: Handle<Channel>,
    length: Arc<AtomicUsize>,
}

pub struct Directory<'a> {
    file: File<'a>,
    id: u64,
    owner: Arc<()>,
}

impl<'a> Client<'a> {
    pub fn statfs(&self, path: &[u8], deadline: u64) -> Result<crate::Statistics, Error> {
        self.start_statistics_at(None, path)?
            .wait(runtime::ipc::WaitMode::Until(deadline))
    }

    pub fn from_channel(task: &'a Handle<Task>, channel: Handle<Channel>) -> Self {
        Self {
            task,
            channel,
            identity: Arc::new(()),
        }
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
        self.start_open_directory(path)?
            .wait(runtime::ipc::WaitMode::Until(deadline))
    }

    /// Flushes every mounted filesystem and reports the first flush failure.
    pub fn start_sync(&self) -> Result<Pending<()>, Error> {
        pending::empty_call(&self.channel, Message::new(protocol::SYNC, &[])?)
    }

    pub fn sync(&self, deadline: u64) -> Result<(), Error> {
        self.start_sync()?
            .wait(runtime::ipc::WaitMode::Until(deadline))
    }

    pub fn metadata(&self, path: &[u8], deadline: u64) -> Result<crate::Metadata, Error> {
        let mut request = Message::new(protocol::METADATA, &[path.len() as u64])?;
        transfer(&mut request, self.path(path)?, Rights::READ)?;
        let (mut request, output) = request::metadata_request(self.task, request)?;
        response::complete_metadata(output, call(&self.channel, &mut request, deadline)?)
    }

    pub fn hard_link(&self, source: &[u8], target: &[u8], deadline: u64) -> Result<(), Error> {
        self.pair_request(protocol::HARD_LINK, source, target, deadline)
    }

    pub fn symlink(&self, target: &[u8], path: &[u8], deadline: u64) -> Result<(), Error> {
        let mut request = self.symlink_message(target, path)?;
        response::complete_empty(call(&self.channel, &mut request, deadline)?)
    }

    pub fn read_link(&self, path: &[u8], deadline: u64) -> Result<alloc::vec::Vec<u8>, Error> {
        let request = self.path_message(protocol::READ_LINK, path)?;
        let (mut request, output) = request::output_request(self.task, request, PAGE_SIZE)?;
        response::complete_read(
            (output, PAGE_SIZE - 1),
            call(&self.channel, &mut request, deadline)?,
        )
    }

    fn open_request(&self, method: u64, path: &[u8], deadline: u64) -> Result<File<'a>, Error> {
        let (mut request, file) =
            self.open_message(method, path, CHANNEL_RIGHTS | Rights::TRANSFER)?;
        response::complete_open(
            (self.task, file),
            call(&self.channel, &mut request, deadline)?,
        )
    }
    fn path_request(&self, method: u64, path: &[u8], deadline: u64) -> Result<(), Error> {
        let mut request = self.path_message(method, path)?;
        response::complete_empty(call(&self.channel, &mut request, deadline)?)
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
        let mut request = self.pair_message(method, source, target)?;
        response::complete_empty(call(&self.channel, &mut request, deadline)?)
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
        let (mut request, output) =
            request::metadata_request(self.task, Message::new(protocol::METADATA, &[])?)?;
        response::complete_metadata(output, call(&self.channel, &mut request, deadline)?)
    }

    pub fn set_attributes(
        &self,
        attributes: crate::Attributes,
        deadline: u64,
    ) -> Result<(), Error> {
        let mut request = request::attributes_request(self.task, attributes)?;
        response::complete_empty(call(&self.channel, &mut request, deadline)?)
    }

    pub fn append(&self, input: &[u8], deadline: u64) -> Result<(usize, usize), Error> {
        if input.is_empty() {
            return Ok((self.size(deadline)?, 0));
        }
        let mut request = self.write_message(protocol::APPEND, 0, input)?;
        let response = call(&self.channel, &mut request, deadline)?;
        response::complete_append((&self.length, input.len()), response)
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
        response::complete_empty(response)
    }

    pub fn write_at(&self, offset: usize, input: &[u8], deadline: u64) -> Result<usize, Error> {
        let length = input.len();
        if length == 0 {
            return Ok(0);
        }
        let mut request = self.write_message(protocol::WRITE, offset, input)?;
        let response = call(&self.channel, &mut request, deadline)?;
        response::complete_write((&self.length, length), response)
    }
    pub fn truncate(&self, length: usize, deadline: u64) -> Result<(), Error> {
        let response = call(
            &self.channel,
            &mut Message::new(protocol::TRUNCATE, &[length as u64])?,
            deadline,
        )?;
        response::complete_truncate((&self.length, length), response)
    }
    /// Returns the huge page size of a huge page backed file, or zero for other files.
    pub fn hugepage_size(&self, deadline: u64) -> Result<usize, Error> {
        let response = call(
            &self.channel,
            &mut Message::new(protocol::HUGEPAGE_INFO, &[])?,
            deadline,
        )?;
        let [size] = *response.words() else {
            return Err(Error::InvalidData);
        };
        usize::try_from(size).map_err(|_| Error::InvalidData)
    }

    /// Materializes a huge page block and returns its frame allocation for mapping.
    pub fn map_huge(
        &self,
        offset: usize,
        deadline: u64,
    ) -> Result<runtime::Handle<runtime::FrameAllocation>, Error> {
        response::complete_map_huge(protocol::call_with_reply(
            &self.channel,
            &mut Message::new(protocol::MAP_HUGEPAGE, &[offset as u64])?,
            deadline,
        )?)
    }

    pub fn size(&self, deadline: u64) -> Result<usize, Error> {
        let response = call(
            &self.channel,
            &mut Message::new(protocol::FILE_INFO, &[])?,
            deadline,
        )?;
        response::complete_size(&self.length, response)
    }

    pub fn read_at(&self, offset: usize, output: &mut [u8], deadline: u64) -> Result<usize, Error> {
        let length = output.len();
        if length == 0 {
            return Ok(0);
        }
        let (mut request, frames) = self.read_message(offset, length)?;
        let count = response::read_count(call(&self.channel, &mut request, deadline)?, length)?;
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
        let mut result = alloc::vec::Vec::new();
        loop {
            let page = self
                .start_read_dir(result.len())?
                .wait(WaitMode::Until(deadline))?;
            if page.is_empty() {
                return Ok(result);
            }
            result.extend(page);
        }
    }
}

/// An open file identity and the state shared with its mapped views.
pub struct MappingSource {
    channel: Handle<Channel>,
    state: Handle<FrameAllocation>,
    pub granule: usize,
    pub device: u64,
    pub inode: u64,
}

impl MappingSource {
    /// Returns the byte length and truncation generation of the file.
    pub fn state(&self) -> Result<(usize, u64), Error> {
        let mut bytes = [0; 16];
        self.state.read(0, &mut bytes)?;
        Ok((
            usize::try_from(u64::from_le_bytes(bytes[..8].try_into().unwrap()))
                .map_err(|_| Error::FileTooLarge)?,
            u64::from_le_bytes(bytes[8..].try_into().unwrap()),
        ))
    }

    pub fn start_map(
        &self,
        offset: usize,
        writable: bool,
    ) -> Result<pending::PendingHugeMap, Error> {
        Ok(pending::Pending::new(
            self.channel.start_call(&mut Message::new(
                protocol::MAP_PAGE,
                &[offset as u64, u64::from(writable)],
            )?)?,
            (),
            |(), response| response::complete_map_huge(response),
        ))
    }

    pub fn map(&self, offset: usize, writable: bool) -> Result<Handle<FrameAllocation>, Error> {
        let deadline = runtime::thread::now_ns()?
            .checked_add(10_000_000_000)
            .ok_or(Error::Unavailable)?;
        self.start_map(offset, writable)?
            .wait(runtime::ipc::WaitMode::Until(deadline))
    }

    pub fn sync(&self) -> Result<(), Error> {
        let deadline = runtime::thread::now_ns()?
            .checked_add(10_000_000_000)
            .ok_or(Error::Unavailable)?;
        pending::empty_call(&self.channel, Message::new(protocol::FSYNC, &[])?)?
            .wait(runtime::ipc::WaitMode::Until(deadline))
    }
}
