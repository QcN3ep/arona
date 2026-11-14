// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use crate::{
    Client, Error,
    protocol::{self, CHANNEL_RIGHTS, call, transfer},
};
use runtime::{Channel, FrameAllocation, Handle, Rights, Task, ipc::Message};

pub struct Control<'a> {
    task: &'a Handle<Task>,
    channel: Handle<Channel>,
}

impl<'a> Control<'a> {
    pub fn start(
        task: &'a Handle<Task>,
        channel: Handle<Channel>,
        frames: Handle<FrameAllocation>,
        length: usize,
        deadline: u64,
    ) -> Result<Self, Error> {
        let mut request = Message::new(protocol::START, &[length as u64])?;
        transfer(&mut request, frames, Rights::READ)?;
        empty(call(&channel, &mut request, deadline)?)?;
        Ok(Self { task, channel })
    }

    pub fn connect(&self, deadline: u64) -> Result<Client<'a>, Error> {
        let (channel, server) = self.task.channel(
            CHANNEL_RIGHTS | Rights::TRANSFER,
            Rights::READ | Rights::WAIT | Rights::TRANSFER,
        )?;
        let mut request = Message::new(protocol::CONNECT, &[])?;
        transfer(&mut request, server, Rights::READ | Rights::WAIT)?;
        empty(call(&self.channel, &mut request, deadline)?)?;
        Ok(Client::from_channel(self.task, channel))
    }

    pub fn mount(
        &self,
        path: &[u8],
        session: devmgr::block::Session,
        start: u64,
        length: u64,
        deadline: u64,
    ) -> Result<(), Error> {
        let mut request = Message::new(protocol::MOUNT, &[path.len() as u64, start, length])?;
        transfer(&mut request, self.path(path)?, Rights::READ)?;
        transfer(&mut request, session.into_channel(), CHANNEL_RIGHTS)?;
        empty(call(&self.channel, &mut request, deadline)?)
    }

    pub fn mount_tmpfs(&self, path: &[u8], deadline: u64) -> Result<(), Error> {
        let mut request = Message::new(protocol::TMPFS, &[path.len() as u64])?;
        transfer(&mut request, self.path(path)?, Rights::READ)?;
        empty(call(&self.channel, &mut request, deadline)?)
    }

    pub fn unmount(&self, path: &[u8], deadline: u64) -> Result<(), Error> {
        let mut request = Message::new(protocol::UNMOUNT, &[path.len() as u64])?;
        transfer(&mut request, self.path(path)?, Rights::READ)?;
        empty(call(&self.channel, &mut request, deadline)?)
    }

    fn path(&self, path: &[u8]) -> Result<Handle<FrameAllocation>, Error> {
        crate::path::components(path)?;
        let mut frames = self
            .task
            .allocate(1, Rights::READ_WRITE | Rights::TRANSFER)?;
        frames.write(0, path)?;
        Ok(frames)
    }
}

fn empty(response: runtime::ipc::ReceivedMessage) -> Result<(), Error> {
    if response.words().is_empty() {
        Ok(())
    } else {
        Err(Error::InvalidData)
    }
}
