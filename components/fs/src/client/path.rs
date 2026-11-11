// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Relative path requests for directory sessions.

use super::pending::{
    complete_directory, complete_open, complete_read, empty_call, input_buffer, metadata_call,
    output_buffer, statistics_call,
};
use super::{Client, Directory, File, PAGE_SIZE, Pending, PendingMetadata, PendingRead};
use crate::{
    Error, OpenOptions,
    protocol::{self, CHANNEL_RIGHTS, transfer},
};
use alloc::vec;
use runtime::{
    Channel, Handle, Rights, Task,
    ipc::{Message, ReceivedMessage},
};

pub enum OpenedNode<'a> {
    File(File<'a>),
    Directory(Directory<'a>),
}

pub type PendingNode<'a> =
    Pending<OpenedNode<'a>, (&'a Handle<Task>, Handle<Channel>, alloc::sync::Arc<()>)>;

impl<'a> Client<'a> {
    pub fn start_statistics_at(
        &self,
        directory: Option<&Directory<'_>>,
        path: &[u8],
    ) -> Result<super::PendingStatistics, Error> {
        let mut request = at(self, protocol::STATFS, directory, 0, &[path.len() as u64])?;
        transfer(&mut request, path_buffer(self.task, path)?, Rights::READ)?;
        statistics_call(self.task, &self.channel, request)
    }

    pub fn start_open_at(
        &self,
        directory: Option<&Directory<'_>>,
        path: &[u8],
        options: OpenOptions,
        follow: bool,
        require_directory: bool,
    ) -> Result<PendingNode<'a>, Error> {
        options.validate()?;
        let (channel, server) = self.task.channel(
            CHANNEL_RIGHTS | Rights::TRANSFER,
            Rights::READ | Rights::WAIT | Rights::TRANSFER,
        )?;
        let [flags, permissions] = options.encode();
        let mut request = if require_directory {
            at(
                self,
                protocol::OPEN_DIRECTORY,
                directory,
                u64::from(follow),
                &[path.len() as u64],
            )?
        } else {
            at(
                self,
                protocol::OPEN,
                directory,
                u64::from(follow),
                &[path.len() as u64, flags, permissions],
            )?
        };
        transfer(&mut request, path_buffer(self.task, path)?, Rights::READ)?;
        transfer(&mut request, server, Rights::READ | Rights::WAIT)?;
        Ok(Pending::new(
            self.channel.start_call(&mut request)?,
            (self.task, channel, alloc::sync::Arc::clone(&self.identity)),
            complete_node,
        ))
    }

    pub fn start_metadata_at(
        &self,
        directory: Option<&Directory<'_>>,
        path: &[u8],
        follow: bool,
    ) -> Result<PendingMetadata, Error> {
        let mut request = at(
            self,
            protocol::METADATA,
            directory,
            u64::from(follow),
            &[path.len() as u64],
        )?;
        transfer(&mut request, path_buffer(self.task, path)?, Rights::READ)?;
        metadata_call(self.task, &self.channel, request)
    }

    pub fn start_mkdir_at(
        &self,
        directory: Option<&Directory<'_>>,
        path: &[u8],
        permissions: u16,
    ) -> Result<Pending<()>, Error> {
        let mut request = at(
            self,
            protocol::MKDIR,
            directory,
            0,
            &[path.len() as u64, u64::from(permissions)],
        )?;
        transfer(&mut request, path_buffer(self.task, path)?, Rights::READ)?;
        empty_call(&self.channel, request)
    }

    pub fn start_unlink_at(
        &self,
        directory: Option<&Directory<'_>>,
        path: &[u8],
        remove_directory: bool,
    ) -> Result<Pending<()>, Error> {
        let mut request = at(
            self,
            protocol::UNLINK,
            directory,
            u64::from(remove_directory),
            &[path.len() as u64],
        )?;
        transfer(&mut request, path_buffer(self.task, path)?, Rights::READ)?;
        empty_call(&self.channel, request)
    }

    pub fn start_rename_at(
        &self,
        source: (Option<&Directory<'_>>, &[u8]),
        target: (Option<&Directory<'_>>, &[u8]),
    ) -> Result<Pending<()>, Error> {
        self.pair_at(protocol::RENAME, source, target, false)
    }

    pub fn start_hard_link_at(
        &self,
        source: (Option<&Directory<'_>>, &[u8]),
        target: (Option<&Directory<'_>>, &[u8]),
        follow: bool,
    ) -> Result<Pending<()>, Error> {
        self.pair_at(protocol::HARD_LINK, source, target, follow)
    }

    fn pair_at(
        &self,
        method: u64,
        source: (Option<&Directory<'_>>, &[u8]),
        target: (Option<&Directory<'_>>, &[u8]),
        follow: bool,
    ) -> Result<Pending<()>, Error> {
        if target
            .0
            .is_some_and(|directory| !alloc::sync::Arc::ptr_eq(&self.identity, &directory.owner))
        {
            return Err(Error::InvalidRequest);
        }
        let mut request = at(
            self,
            method,
            source.0,
            u64::from(follow),
            &[
                source.1.len() as u64,
                target.1.len() as u64,
                target.0.map_or(0, |directory| directory.id),
            ],
        )?;
        transfer(
            &mut request,
            path_buffer(self.task, source.1)?,
            Rights::READ,
        )?;
        transfer(
            &mut request,
            path_buffer(self.task, target.1)?,
            Rights::READ,
        )?;
        empty_call(&self.channel, request)
    }

    pub fn start_symlink_at(
        &self,
        target: &[u8],
        directory: Option<&Directory<'_>>,
        path: &[u8],
    ) -> Result<Pending<()>, Error> {
        let mut request = at(
            self,
            protocol::SYMLINK,
            directory,
            0,
            &[target.len() as u64, path.len() as u64],
        )?;
        transfer(&mut request, path_buffer(self.task, target)?, Rights::READ)?;
        transfer(&mut request, path_buffer(self.task, path)?, Rights::READ)?;
        empty_call(&self.channel, request)
    }

    pub fn start_read_link_at(
        &self,
        directory: Option<&Directory<'_>>,
        path: &[u8],
    ) -> Result<PendingRead, Error> {
        let output = output_buffer(self.task, PAGE_SIZE)?;
        let mut request = at(
            self,
            protocol::READ_LINK,
            directory,
            0,
            &[path.len() as u64],
        )?;
        transfer(&mut request, path_buffer(self.task, path)?, Rights::READ)?;
        transfer(
            &mut request,
            output.try_clone(Rights::WRITE | Rights::TRANSFER)?,
            Rights::WRITE,
        )?;
        Ok(Pending::new(
            self.channel.start_call(&mut request)?,
            (output, PAGE_SIZE - 1),
            complete_read,
        ))
    }
}

impl Directory<'_> {
    pub fn start_path(&self) -> Result<PendingRead, Error> {
        let output = output_buffer(self.file.task, PAGE_SIZE)?;
        let mut request = Message::new(protocol::DIRECTORY_PATH, &[])?;
        transfer(
            &mut request,
            output.try_clone(Rights::WRITE | Rights::TRANSFER)?,
            Rights::WRITE,
        )?;
        Ok(Pending::new(
            self.file.channel.start_call(&mut request)?,
            (output, PAGE_SIZE - 1),
            complete_read,
        ))
    }
}

fn at(
    client: &Client<'_>,
    method: u64,
    directory: Option<&Directory<'_>>,
    flags: u64,
    words: &[u64],
) -> Result<Message, Error> {
    if directory
        .is_some_and(|directory| !alloc::sync::Arc::ptr_eq(&client.identity, &directory.owner))
    {
        return Err(Error::InvalidRequest);
    }
    let mut data = vec![method, directory.map_or(0, |directory| directory.id), flags];
    data.extend_from_slice(words);
    Ok(Message::new(protocol::RELATIVE, &data)?)
}

fn path_buffer(
    task: &Handle<Task>,
    path: &[u8],
) -> Result<Handle<runtime::FrameAllocation>, Error> {
    if path.is_empty() || path.len() >= PAGE_SIZE || path.contains(&0) {
        return Err(Error::InvalidRequest);
    }
    input_buffer(task, path)
}

fn complete_node<'a>(
    state: (&'a Handle<Task>, Handle<Channel>, alloc::sync::Arc<()>),
    response: ReceivedMessage,
) -> Result<OpenedNode<'a>, Error> {
    if response.words().len() == 2 {
        complete_directory(state, response).map(OpenedNode::Directory)
    } else {
        complete_open((state.0, state.1), response).map(OpenedNode::File)
    }
}
