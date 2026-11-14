// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use crate::{
    Client, Error,
    client::Pending,
    protocol::{self, CHANNEL_RIGHTS, call, response_result, transfer},
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

    /// Mounts an in-memory filesystem; zero capacity selects the default.
    pub fn mount_tmpfs(&self, path: &[u8], capacity: usize, deadline: u64) -> Result<(), Error> {
        let mut request = Message::new(protocol::TMPFS, &[path.len() as u64, capacity as u64])?;
        transfer(&mut request, self.path(path)?, Rights::READ)?;
        empty(call(&self.channel, &mut request, deadline)?)
    }

    pub fn unmount(&self, path: &[u8], deadline: u64) -> Result<(), Error> {
        let mut request = Message::new(protocol::UNMOUNT, &[path.len() as u64])?;
        transfer(&mut request, self.path(path)?, Rights::READ)?;
        empty(call(&self.channel, &mut request, deadline)?)
    }

    /// Opens a dedicated mount-control channel for in-memory volumes.
    ///
    /// The service accepts only tmpfs mount creation and in-memory unmounting on
    /// this channel. It cannot mount or unmount disk volumes or the service-owned
    /// startup mounts.
    pub fn mount_control(&self, deadline: u64) -> Result<MountControl<'a>, Error> {
        let (channel, server) = self.task.channel(
            CHANNEL_RIGHTS | Rights::DUPLICATE | Rights::TRANSFER,
            Rights::READ_WRITE | Rights::WAIT | Rights::TRANSFER,
        )?;
        let mut request = Message::new(protocol::CONNECT_MOUNT, &[])?;
        transfer(&mut request, server, Rights::READ_WRITE | Rights::WAIT)?;
        empty(call(&self.channel, &mut request, deadline)?)?;
        Ok(MountControl {
            task: self.task,
            channel,
        })
    }

    fn path(&self, path: &[u8]) -> Result<Handle<FrameAllocation>, Error> {
        path_frames(self.task, path)
    }
}

/// Mount and unmount requests for in-memory volumes issued by a client such as the
/// Linux compatibility layer.
///
/// The service creates the dedicated channel for [`Control::mount_control`] and
/// derives mount ownership from that channel, so a client cannot forge a
/// service-owned mount. Only mounts created through this handle can be unmounted
/// through it.
pub struct MountControl<'a> {
    task: &'a Handle<Task>,
    channel: Handle<Channel>,
}

impl<'a> MountControl<'a> {
    pub fn from_channel(task: &'a Handle<Task>, channel: Handle<Channel>) -> Result<Self, Error> {
        if !channel
            .info()?
            .rights
            .contains(Rights::READ_WRITE | Rights::WAIT)
        {
            return Err(Error::InvalidRequest);
        }
        Ok(Self { task, channel })
    }

    pub fn start_mount_tmpfs(&self, path: &[u8]) -> Result<Pending<()>, Error> {
        let mut request = Message::new(protocol::MOUNT_TMPFS, &[path.len() as u64])?;
        transfer(&mut request, path_frames(self.task, path)?, Rights::READ)?;
        empty_call(&self.channel, request)
    }

    pub fn start_unmount_tmpfs(&self, path: &[u8]) -> Result<Pending<()>, Error> {
        let mut request = Message::new(protocol::UNMOUNT_TMPFS, &[path.len() as u64])?;
        transfer(&mut request, path_frames(self.task, path)?, Rights::READ)?;
        empty_call(&self.channel, request)
    }

    /// Duplicates the mount-control channel so a pending request can own it.
    pub fn try_clone(&self) -> Result<Self, Error> {
        Ok(Self {
            task: self.task,
            channel: self
                .channel
                .try_clone(Rights::READ_WRITE | Rights::WAIT | Rights::DUPLICATE)?,
        })
    }

    pub fn into_channel(self) -> Handle<Channel> {
        self.channel
    }
}

fn path_frames(task: &Handle<Task>, path: &[u8]) -> Result<Handle<FrameAllocation>, Error> {
    crate::path::components(path)?;
    let mut frames = task.allocate(1, Rights::READ_WRITE | Rights::TRANSFER)?;
    frames.write(0, path)?;
    Ok(frames)
}

fn empty(response: runtime::ipc::ReceivedMessage) -> Result<(), Error> {
    if response.words().is_empty() {
        Ok(())
    } else {
        Err(Error::InvalidData)
    }
}

fn empty_call(channel: &Handle<Channel>, mut request: Message) -> Result<Pending<()>, Error> {
    Ok(Pending::new(
        channel.start_call(&mut request)?,
        (),
        |(), response| empty(response_result(response)?),
    ))
}
