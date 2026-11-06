// SPDX-License-Identifier: MPL-2.0

//! VirtIO 随机数设备的熵读取与 DMA 生命周期。

pub mod service;
mod session;

pub use session::{PendingOpen, PendingRead, Session};

use crate::{
    Error,
    device::DeviceDescription,
    virtio::{
        self, Transport,
        queue::{Descriptor, Queue},
    },
};
use runtime::{
    Device, DmaDomain, Event, Handle, Task,
    device::InterruptBinding,
    dma::{DmaBuffer, DmaDirection},
    ipc::{self, WaitItem, WaitMode},
};

pub const MAX_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug)]
pub struct Configuration {
    pub(crate) id: u32,
    transport: virtio::Configuration,
}

pub struct Rng<'a> {
    transport: Transport<'a>,
    interrupt: InterruptBinding<'a>,
    queue: Queue<'a>,
    buffer: DmaBuffer<'a>,
    domain: &'a Handle<DmaDomain>,
    requested: usize,
    running: bool,
}

pub fn probe(device: &DeviceDescription<'_>) -> Result<Option<Configuration>, Error> {
    Ok(
        virtio::probe(device, virtio::RNG_DEVICE_TYPE)?.map(|transport| Configuration {
            id: device.info().id,
            transport,
        }),
    )
}

impl<'a> Rng<'a> {
    pub fn new(
        configuration: Configuration,
        grant: &'a Handle<Device>,
        domain: &'a Handle<DmaDomain>,
        event: &'a Handle<Event>,
        task: &Handle<Task>,
    ) -> Result<Self, Error> {
        let mut transport = Transport::open(configuration.transport, grant)?;
        transport.negotiate(0)?;
        let queue = Queue::new(domain, task, &mut transport)?;
        let buffer = domain.allocate(task, 1, DmaDirection::FromDevice)?;
        match ipc::wait_many(&[WaitItem::new(event, 1)], WaitMode::Poll) {
            Ok(_) | Err(runtime::Error::Kernel(runtime::KernelError::WouldBlock)) => {}
            Err(error) => return Err(error.into()),
        }
        let interrupt = grant.bind_interrupt(configuration.id, 0, event, 1)?;
        let mut rng = Self {
            transport,
            interrupt,
            queue,
            buffer,
            domain,
            requested: 0,
            running: false,
        };
        domain.resume()?;
        rng.running = true;
        rng.transport.start()?;
        Ok(rng)
    }

    pub fn request(&mut self, length: usize) -> Result<(), Error> {
        if !self.running {
            return Err(Error::Unavailable);
        }
        if length == 0 || length > MAX_BYTES {
            return Err(Error::InvalidRequest);
        }
        if self.requested != 0 {
            return Err(Error::Busy);
        }
        self.transport.check()?;
        self.queue.publish(
            0,
            &[Descriptor {
                address: self.buffer.bus_address().ok_or(Error::Unavailable)?,
                length: length as u32,
                writable: true,
            }],
        )?;
        self.requested = length;
        self.transport.notify()
    }

    pub fn poll(&mut self, output: &mut [u8]) -> Result<Option<usize>, Error> {
        if !self.running {
            return Err(Error::Unavailable);
        }
        if output.len() < self.requested {
            return Err(Error::InvalidRequest);
        }
        self.transport.check()?;
        let Some((head, length)) = self.queue.complete()? else {
            return Ok(None);
        };
        if head != 0 || length as usize > self.requested {
            return Err(Error::DeviceFault);
        }
        self.buffer.read(0, &mut output[..length as usize])?;
        self.requested = 0;
        Ok(Some(length as usize))
    }

    pub fn acknowledge_interrupt(&mut self) -> Result<(), Error> {
        if self.transport.interrupt_status()? & 2 != 0 {
            return Err(Error::DeviceFault);
        }
        Ok(self.interrupt.acknowledge()?)
    }

    pub fn stop(&mut self) -> Result<(), Error> {
        if !self.running {
            return Ok(());
        }
        self.transport.reset()?;
        // SAFETY: 设备已复位，不能再访问该域中的 DMA 缓冲。
        self.domain.quiesce()?;
        self.running = false;
        self.requested = 0;
        self.interrupt.unbind()?;
        Ok(())
    }
}

impl Drop for Rng<'_> {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
