// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Device description queries and session handoff.

use runtime::{
    Call, Channel, DeviceSet, Handle, KernelError, Reply, Rights, Task,
    device::{DeviceInfo, topic},
    ipc::{Message, ReceivedMessage, Transfer, WaitMode},
};

use crate::{
    Error,
    protocol::{BUSY, INVALID_REQUEST, SUCCESS, UNAVAILABLE},
};

const NEXT: u64 = 2;

pub type Pending<T, S = ()> = runtime::ipc::PendingCall<T, S, Error>;

pub(crate) fn open_call(
    task: &Handle<Task>,
    service: &Handle<Channel>,
    device: u32,
) -> Result<(Handle<Call>, Handle<Channel>), Error> {
    open_call_partition(task, service, device, 0)
}

pub(crate) fn open_call_partition(
    task: &Handle<Task>,
    service: &Handle<Channel>,
    device: u32,
    partition: u32,
) -> Result<(Handle<Call>, Handle<Channel>), Error> {
    let rights = CHANNEL_RIGHTS | Rights::TRANSFER;
    let (channel, server) = task.channel(rights, rights)?;
    let mut request = Message::new(OPEN, &[device.into(), partition.into()])?;
    request
        .push(Transfer {
            handle: server.into(),
            rights,
        })
        .map_err(|_| Error::InvalidRequest)?;
    Ok((service.start_call(&mut request)?, channel))
}

pub(crate) fn complete_open(
    channel: Handle<Channel>,
    response: ReceivedMessage,
) -> Result<Handle<Channel>, Error> {
    crate::protocol::empty(crate::protocol::response_result(response)?)?;
    Ok(channel)
}

#[derive(Clone, Copy)]
#[repr(u64)]
pub enum DriverKind {
    Uart = 1,
    Block = 2,
    Rng = 3,
    Rtc = 4,
    Power = 5,
    Net = 6,
}

pub fn next(
    service: &Handle<Channel>,
    kind: DriverKind,
    after: u32,
    deadline: u64,
) -> Result<Option<u32>, Error> {
    let response = service
        .call(
            &mut Message::new(NEXT, &[kind as u64, after.into()])?,
            Some(deadline),
        )
        .map_err(Error::Call)?;
    complete_next(after, response)
}

pub fn start_next(
    service: &Handle<Channel>,
    kind: DriverKind,
    after: u32,
) -> Result<Pending<Option<u32>, u32>, Error> {
    let call = service.start_call(&mut Message::new(NEXT, &[kind as u64, after.into()])?)?;
    Ok(Pending::new(call, after, complete_next))
}

fn complete_next(after: u32, response: ReceivedMessage) -> Result<Option<u32>, Error> {
    if response.tag != SUCCESS || !response.handles().is_empty() {
        return Err(runtime::Error::Protocol.into());
    }
    let [id] = *response.words() else {
        return Err(runtime::Error::Protocol.into());
    };
    let id = u32::try_from(id).map_err(|_| runtime::Error::Protocol)?;
    if id != 0 && id <= after {
        return Err(runtime::Error::Protocol.into());
    }
    Ok((id != 0).then_some(id))
}

pub struct DiscoveryRequest {
    pub kind: DriverKind,
    pub after: u32,
    reply: Handle<Reply>,
}

impl DiscoveryRequest {
    pub fn decode(message: &mut ReceivedMessage) -> Result<Option<Self>, Error> {
        if message.tag != NEXT {
            return Ok(None);
        }
        let Some(reply) = message.take_reply() else {
            return Ok(None);
        };
        let (kind, after) = match message.words() {
            [kind @ 1..=6, after] if *after <= u32::MAX as u64 && message.handles().is_empty() => (
                match *kind {
                    1 => DriverKind::Uart,
                    2 => DriverKind::Block,
                    3 => DriverKind::Rng,
                    4 => DriverKind::Rtc,
                    5 => DriverKind::Power,
                    _ => DriverKind::Net,
                },
                *after as u32,
            ),
            _ => {
                reject(&reply, INVALID_REQUEST)?;
                return Ok(None);
            }
        };
        Ok(Some(Self { kind, after, reply }))
    }

    pub fn respond(self, id: Option<u32>) -> Result<(), Error> {
        match self
            .reply
            .send(&mut Message::new(SUCCESS, &[id.unwrap_or(0).into()])?)
        {
            Ok(()) | Err(runtime::Error::Kernel(KernelError::InvalidState)) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

pub(crate) const OPEN: u64 = 1;
pub(crate) const CHANNEL_RIGHTS: Rights = Rights::READ_WRITE.union(Rights::WAIT);

pub struct OpenRequest {
    pub device: u32,
    pub partition: u32,
    pub(crate) channel: Handle<Channel>,
    pub(crate) reply: Handle<Reply>,
}

impl OpenRequest {
    pub fn decode(mut request: ReceivedMessage) -> Result<Option<Self>, Error> {
        let reply = match request.take_reply() {
            Some(reply) => reply,
            None => match request
                .take_handle(0)
                .and_then(|handle| handle.downcast().ok())
            {
                Some(reply) => reply,
                None => return Ok(None),
            },
        };
        if !reply.info()?.rights.contains(Rights::WRITE) {
            return Ok(None);
        }
        let (device, partition) = match (request.tag, request.words()) {
            (OPEN, [id, partition])
                if *id <= u64::from(u32::MAX)
                    && *partition <= u64::from(u32::MAX)
                    && request.handles().len() == 1 =>
            {
                (*id as u32, *partition as u32)
            }
            _ => {
                reject(&reply, INVALID_REQUEST)?;
                return Ok(None);
            }
        };
        let Ok(channel) = request.take_handle(0).unwrap().downcast::<Channel>() else {
            reject(&reply, INVALID_REQUEST)?;
            return Ok(None);
        };
        if !channel.info()?.rights.contains(CHANNEL_RIGHTS) {
            reject(&reply, INVALID_REQUEST)?;
            return Ok(None);
        }
        Ok(Some(Self {
            device,
            partition,
            channel,
            reply,
        }))
    }

    pub fn reject(self, error: Error) -> Result<(), Error> {
        reject(&self.reply, crate::protocol::error_status(error))
    }

    pub fn forward(self, target: &Handle<Channel>) -> Result<(), Error> {
        if !self.channel.info()?.rights.contains(Rights::TRANSFER) {
            return reject(&self.reply, INVALID_REQUEST);
        }
        let mut message = Message::new(OPEN, &[u64::from(self.device), u64::from(self.partition)])?;
        for (handle, rights) in [
            (self.reply.into(), Rights::WRITE),
            (self.channel.into(), CHANNEL_RIGHTS),
        ] {
            message
                .push(Transfer { handle, rights })
                .map_err(|_| runtime::Error::Protocol)?;
        }
        if let Err(error) = target.send(&mut message, WaitMode::Poll) {
            drop(message.pop());
            let reply = message.pop().unwrap().handle.downcast::<Reply>().unwrap();
            reject(
                &reply,
                if error == runtime::Error::Kernel(KernelError::WouldBlock) {
                    BUSY
                } else {
                    UNAVAILABLE
                },
            )?;
        }
        Ok(())
    }
}

fn reject(reply: &Handle<Reply>, status: u64) -> Result<(), Error> {
    match reply.send(&mut Message::new(status, &[])?) {
        Ok(()) | Err(runtime::Error::Kernel(KernelError::InvalidState)) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

mod pci;
pub use pci::PciBar;

pub struct DeviceDescription<'a> {
    catalog: &'a Handle<DeviceSet>,
    info: DeviceInfo,
}

impl<'a> DeviceDescription<'a> {
    pub fn new(catalog: &'a Handle<DeviceSet>, id: u32) -> Result<Self, Error> {
        Ok(Self {
            catalog,
            info: catalog.describe(id)?,
        })
    }

    pub fn info(&self) -> DeviceInfo {
        self.info
    }

    pub(crate) fn pci_config(&self, output: &mut [u8; 4096]) -> Result<(), Error> {
        if self.catalog.configuration(self.info.id, output)? != output.len() {
            return Err(runtime::Error::Protocol.into());
        }
        Ok(())
    }

    pub fn pci_bar(&self, index: usize) -> Result<PciBar, Error> {
        if index >= 6 {
            return Err(Error::InvalidConfiguration);
        }
        let name = [b'b', b'a', b'r', b'0' + index as u8];
        let mut bytes = [0; PciBar::SIZE];
        let bytes = self
            .property("pci", core::str::from_utf8(&name).unwrap(), &mut bytes)?
            .ok_or(Error::Unsupported)?;
        PciBar::decode(bytes).ok_or(Error::InvalidConfiguration)
    }

    pub fn matches(&self, namespace: &str, identity: &str) -> Result<bool, Error> {
        let mut buffer = [0; 128];
        for index in 0..self.info.identities {
            if self
                .catalog
                .read(self.info.id, topic::IDENTITY_NAMESPACE, index, &mut buffer)?
                != namespace.as_bytes()
            {
                continue;
            }
            if self
                .catalog
                .read(self.info.id, topic::IDENTITY, index, &mut buffer)?
                == identity.as_bytes()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn property<'b>(
        &self,
        namespace: &str,
        name: &str,
        output: &'b mut [u8],
    ) -> Result<Option<&'b [u8]>, Error> {
        let mut buffer = [0; 128];
        for index in 0..self.info.properties {
            if self
                .catalog
                .read(self.info.id, topic::PROPERTY_NAMESPACE, index, &mut buffer)?
                != namespace.as_bytes()
                || self
                    .catalog
                    .read(self.info.id, topic::PROPERTY_NAME, index, &mut buffer)?
                    != name.as_bytes()
            {
                continue;
            }
            return Ok(Some(self.catalog.read(
                self.info.id,
                topic::PROPERTY_VALUE,
                index,
                output,
            )?));
        }
        Ok(None)
    }

    pub(crate) fn resolve(&self, namespace: &str, name: &str, value: &[u8]) -> Result<Self, Error> {
        let mut cursor = 0;
        let mut found = None;
        let mut buffer = [0; 128];
        while let Some(id) = self.catalog.next(cursor)? {
            let device = Self::new(self.catalog, id)?;
            if device.property(namespace, name, &mut buffer)? == Some(value) {
                if found.is_some() {
                    return Err(Error::InvalidConfiguration);
                }
                found = Some(device);
            }
            cursor = id;
        }
        found.ok_or(Error::InvalidConfiguration)
    }
}
