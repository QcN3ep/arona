// SPDX-License-Identifier: MPL-2.0

//! Device description queries and session handoff.

use runtime::{
    Call, Channel, DeviceSet, Handle, KernelError, Reply, Rights, Task,
    device::{DeviceField, DeviceInfo},
    ipc::{Message, ReceivedMessage, Transfer, WaitMode},
};

use crate::Error;

const NEXT: u64 = 2;

pub type Pending<T, S = ()> = runtime::ipc::PendingCall<T, S, Error>;

pub(crate) fn open_call(
    task: &Handle<Task>,
    service: &Handle<Channel>,
    device: u32,
) -> Result<(Handle<Call>, Handle<Channel>), Error> {
    let rights = CHANNEL_RIGHTS | Rights::TRANSFER;
    let (channel, server) = task.channel(rights, rights)?;
    let mut request = Message::new(OPEN, &[device.into()])?;
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
    if !response.handles().is_empty() || !response.words().is_empty() {
        return Err(runtime::Error::Protocol.into());
    }
    match response.tag {
        SUCCESS => Ok(channel),
        INVALID_REQUEST => Err(Error::InvalidRequest),
        BUSY => Err(Error::Busy),
        UNSUPPORTED => Err(Error::Unsupported),
        INVALID_CONFIGURATION => Err(Error::InvalidConfiguration),
        CLOCK_UNAVAILABLE => Err(Error::ClockUnavailable),
        UNAVAILABLE => Err(Error::Unavailable),
        _ => Err(runtime::Error::Protocol.into()),
    }
}

#[derive(Clone, Copy)]
#[repr(u64)]
pub enum DriverKind {
    Uart = 1,
    Block = 2,
    Rng = 3,
    Rtc = 4,
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
            [kind @ 1..=4, after] if *after <= u32::MAX as u64 && message.handles().is_empty() => (
                match *kind {
                    1 => DriverKind::Uart,
                    2 => DriverKind::Block,
                    3 => DriverKind::Rng,
                    _ => DriverKind::Rtc,
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
pub(crate) const SUCCESS: u64 = 0;
pub(crate) const INVALID_REQUEST: u64 = 1;
pub(crate) const BUSY: u64 = 2;
pub(crate) const UNSUPPORTED: u64 = 3;
pub(crate) const INVALID_CONFIGURATION: u64 = 4;
pub(crate) const CLOCK_UNAVAILABLE: u64 = 5;
pub(crate) const UNAVAILABLE: u64 = 6;
pub(crate) const DENIED: u64 = 7;

pub struct OpenRequest {
    pub device: u32,
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
        let device = match (request.tag, request.words()) {
            (OPEN, [id]) if *id <= u64::from(u32::MAX) && request.handles().len() == 1 => {
                *id as u32
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
            channel,
            reply,
        }))
    }

    pub fn reject(self, error: Error) -> Result<(), Error> {
        reject(
            &self.reply,
            match error {
                Error::Busy | Error::Runtime(runtime::Error::Kernel(KernelError::Busy)) => BUSY,
                Error::Unsupported => UNSUPPORTED,
                Error::InvalidConfiguration => INVALID_CONFIGURATION,
                Error::ClockUnavailable => CLOCK_UNAVAILABLE,
                _ => UNAVAILABLE,
            },
        )
    }

    pub fn forward(self, target: &Handle<Channel>) -> Result<(), Error> {
        if !self.channel.info()?.rights.contains(Rights::TRANSFER) {
            return reject(&self.reply, INVALID_REQUEST);
        }
        let mut message = Message::new(OPEN, &[u64::from(self.device)])?;
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
        Ok(self.catalog.pci_config(self.info.id, output)?)
    }

    pub(crate) fn pci_bar(&self, index: usize) -> Result<runtime::device::PciBar, Error> {
        Ok(self.catalog.pci_bar(self.info.id, index)?)
    }

    pub fn matches(&self, namespace: &str, identity: &str) -> Result<bool, Error> {
        let mut buffer = [0; 128];
        for index in 0..self.info.identities {
            if self.catalog.read(
                self.info.id,
                DeviceField::IdentityNamespace,
                index,
                &mut buffer,
            )? != namespace.as_bytes()
            {
                continue;
            }
            if self
                .catalog
                .read(self.info.id, DeviceField::Identity, index, &mut buffer)?
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
            if self.catalog.read(
                self.info.id,
                DeviceField::PropertyNamespace,
                index,
                &mut buffer,
            )? != namespace.as_bytes()
                || self
                    .catalog
                    .read(self.info.id, DeviceField::PropertyName, index, &mut buffer)?
                    != name.as_bytes()
            {
                continue;
            }
            return Ok(Some(self.catalog.read(
                self.info.id,
                DeviceField::PropertyValue,
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
