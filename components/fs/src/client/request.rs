// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Request encoding and buffers shared by blocking and asynchronous sessions.

use super::{Client, File, PAGE_SIZE};
use crate::{
    Error,
    protocol::{self, transfer},
};
use runtime::{Channel, FrameAllocation, Handle, Rights, Task, ipc::Message};

impl Client<'_> {
    pub(super) fn open_message(
        &self,
        method: u64,
        path: &[u8],
        rights: Rights,
    ) -> Result<(Message, Handle<Channel>), Error> {
        let frames = self.path(path)?;
        let (channel, server) = self
            .task
            .channel(rights, Rights::READ | Rights::WAIT | Rights::TRANSFER)?;
        let mut request = Message::new(method, &[path.len() as u64])?;
        transfer(&mut request, frames, Rights::READ)?;
        transfer(&mut request, server, Rights::READ | Rights::WAIT)?;
        Ok((request, channel))
    }

    pub(super) fn path_message(&self, method: u64, path: &[u8]) -> Result<Message, Error> {
        let mut request = Message::new(method, &[path.len() as u64])?;
        transfer(&mut request, self.path(path)?, Rights::READ)?;
        Ok(request)
    }

    pub(super) fn pair_message(
        &self,
        method: u64,
        source: &[u8],
        target: &[u8],
    ) -> Result<Message, Error> {
        let mut request = Message::new(method, &[source.len() as u64, target.len() as u64])?;
        transfer(&mut request, self.path(source)?, Rights::READ)?;
        transfer(&mut request, self.path(target)?, Rights::READ)?;
        Ok(request)
    }

    pub(super) fn symlink_message(&self, target: &[u8], path: &[u8]) -> Result<Message, Error> {
        if target.is_empty() || target.len() >= PAGE_SIZE || target.contains(&0) {
            return Err(Error::InvalidRequest);
        }
        let mut request =
            Message::new(protocol::SYMLINK, &[target.len() as u64, path.len() as u64])?;
        transfer(&mut request, input_buffer(self.task, target)?, Rights::READ)?;
        transfer(&mut request, self.path(path)?, Rights::READ)?;
        Ok(request)
    }
}

impl File<'_> {
    pub(super) fn read_message(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<(Message, Handle<runtime::FrameAllocation>), Error> {
        output_request(
            self.task,
            Message::new(protocol::READ, &[offset as u64, length as u64])?,
            length,
        )
    }

    pub(super) fn write_message(
        &self,
        method: u64,
        offset: usize,
        input: &[u8],
    ) -> Result<Message, Error> {
        let mut request = Message::new(method, &[offset as u64, input.len() as u64])?;
        transfer(&mut request, input_buffer(self.task, input)?, Rights::READ)?;
        Ok(request)
    }
}

pub(super) fn output_request(
    task: &runtime::Handle<runtime::Task>,
    mut request: Message,
    length: usize,
) -> Result<(Message, Handle<runtime::FrameAllocation>), Error> {
    let frames = output_buffer(task, length)?;
    transfer(
        &mut request,
        frames.try_clone(Rights::WRITE | Rights::TRANSFER)?,
        Rights::WRITE,
    )?;
    Ok((request, frames))
}

pub(super) fn input_buffer(
    task: &Handle<Task>,
    bytes: &[u8],
) -> Result<Handle<FrameAllocation>, Error> {
    let mut frames = task.allocate(
        bytes.len().div_ceil(PAGE_SIZE).max(1) as u64,
        Rights::READ_WRITE | Rights::TRANSFER,
    )?;
    frames.write(0, bytes)?;
    Ok(frames)
}

pub(super) fn output_buffer(
    task: &Handle<Task>,
    length: usize,
) -> Result<Handle<FrameAllocation>, Error> {
    Ok(task.allocate(
        length.div_ceil(PAGE_SIZE).max(1) as u64,
        Rights::READ_WRITE | Rights::TRANSFER | Rights::DUPLICATE,
    )?)
}

pub(super) fn metadata_request(
    task: &Handle<Task>,
    request: Message,
) -> Result<(Message, Handle<FrameAllocation>), Error> {
    output_request(task, request, crate::Metadata::SIZE)
}

pub(super) fn attributes_request(
    task: &Handle<Task>,
    attributes: crate::Attributes,
) -> Result<Message, Error> {
    let mut request = Message::new(protocol::SET_ATTRIBUTES, &[])?;
    transfer(
        &mut request,
        input_buffer(task, &attributes.encode())?,
        Rights::READ,
    )?;
    Ok(request)
}
