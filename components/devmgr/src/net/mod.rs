// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Copied Ethernet frames, link configuration, and modern VirtIO net DMA ownership.

pub mod service;
mod session;

pub use session::{PendingOpen, PendingReceive, Session};

use alloc::vec::Vec;
use runtime::{
    Device, DmaDomain, Event, Handle, Task,
    device::InterruptBinding,
    dma::{DmaBuffer, DmaDirection},
    ipc::{self, WaitItem, WaitMode},
    thread,
};

use crate::{
    Error,
    device::DeviceDescription,
    virtio::{
        self, Transport,
        queue::{Descriptor, Queue},
    },
};

pub const MAX_FRAME: usize = 1518;
pub const MIN_FRAME: usize = 14;
const HEADER: usize = 12;
const SLOT: usize = 2048;
const MAX_SLOTS: usize = 32;
const MAC: u64 = 1 << 5;
const MTU: u64 = 1 << 3;
const STATUS: u64 = 1 << 16;

#[derive(Clone, Copy, Debug)]
pub struct Configuration {
    pub(crate) id: u32,
    transport: virtio::Configuration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Info {
    pub mac: [u8; 6],
    pub mtu: u16,
    pub link_up: bool,
    pub sequence: u64,
}

impl Info {
    /// Maximum Ethernet frame size, including one VLAN tag and excluding FCS.
    pub fn max_frame(self) -> usize {
        usize::from(self.mtu) + 18
    }
}

pub struct Net<'a> {
    transport: Transport<'a>,
    interrupt: InterruptBinding<'a>,
    rx: Queue<'a>,
    tx: Queue<'a>,
    receive: DmaBuffer<'a>,
    transmit: DmaBuffer<'a>,
    busy: Vec<bool>,
    rx_slots: usize,
    domain: &'a Handle<DmaDomain>,
    features: u64,
    info: Info,
    running: bool,
}

pub fn probe(device: &DeviceDescription<'_>) -> Result<Option<Configuration>, Error> {
    Ok(
        virtio::probe(device, virtio::NET_DEVICE_TYPE)?.map(|transport| Configuration {
            id: device.info().id,
            transport,
        }),
    )
}

impl<'a> Net<'a> {
    pub fn new(
        configuration: Configuration,
        grant: &'a Handle<Device>,
        domain: &'a Handle<DmaDomain>,
        event: &'a Handle<Event>,
        task: &Handle<Task>,
    ) -> Result<Self, Error> {
        let mut transport = Transport::open(configuration.transport, grant)?;
        let features = transport.negotiate(MAC | MTU | STATUS)?;
        if features & MAC == 0 {
            return Err(Error::Unsupported);
        }
        let info = configuration_info(&mut transport, features)?;
        let rx = Queue::new(domain, task, &mut transport, 0)?;
        let tx = Queue::new(domain, task, &mut transport, 1)?;
        let rx_slots = rx.size().min(MAX_SLOTS);
        let tx_slots = tx.size().min(MAX_SLOTS);
        let receive = domain.allocate(
            task,
            (rx_slots * SLOT).div_ceil(4096) as u64,
            DmaDirection::FromDevice,
        )?;
        let transmit = domain.allocate(
            task,
            (tx_slots * SLOT).div_ceil(4096) as u64,
            DmaDirection::ToDevice,
        )?;
        match ipc::wait_many(&[WaitItem::new(event, 1)], WaitMode::Poll) {
            Ok(_) | Err(runtime::Error::Kernel(runtime::KernelError::WouldBlock)) => {}
            Err(error) => return Err(error.into()),
        }
        let interrupt = grant.bind_interrupt(configuration.id, 0, event, 1)?;
        let mut net = Self {
            transport,
            interrupt,
            rx,
            tx,
            receive,
            transmit,
            busy: alloc::vec![false; tx_slots],
            rx_slots,
            domain,
            features,
            info,
            running: false,
        };
        for slot in 0..rx_slots {
            net.replenish(slot as u16)?;
        }
        domain.resume()?;
        net.running = true;
        net.transport.start()?;
        net.rx.notify(&mut net.transport)?;
        Ok(net)
    }

    pub fn info(&self) -> Result<Info, Error> {
        self.check_running()?;
        Ok(self.info)
    }

    pub fn transmit_capacity(&self) -> usize {
        self.busy.len()
    }

    /// Copy and publish a frame. The returned slot remains owned until completion or stop.
    /// Busy and invalid requests have not published a frame.
    pub fn transmit(&mut self, frame: &[u8]) -> Result<u16, Error> {
        self.check_running()?;
        if !(MIN_FRAME..=self.info.max_frame()).contains(&frame.len()) {
            return Err(Error::InvalidRequest);
        }
        if !self.info.link_up {
            return Err(Error::Unavailable);
        }
        self.transport.check()?;
        let slot = self.busy.iter().position(|busy| !busy).ok_or(Error::Busy)?;
        let offset = (slot * SLOT) as u64;
        let mut bytes = [0; HEADER + MAX_FRAME];
        bytes[HEADER..HEADER + frame.len()].copy_from_slice(frame);
        let length = HEADER + frame.len().max(60);
        self.transmit.write(offset, &bytes[..length])?;
        self.tx.publish(
            slot as u16,
            &[Descriptor {
                address: self.transmit.bus_address().ok_or(Error::Unavailable)? + offset,
                length: length as u32,
                writable: false,
            }],
        )?;
        self.busy[slot] = true;
        self.tx.notify(&mut self.transport)?;
        Ok(slot as u16)
    }

    /// Collect a TX slot after the device has finished reading its frame.
    /// This completion does not acknowledge receipt by a network peer.
    pub fn poll_transmit(&mut self) -> Result<Option<u16>, Error> {
        self.check_running()?;
        self.transport.check()?;
        let Some((slot, length)) = self.tx.complete()? else {
            return Ok(None);
        };
        let busy = self
            .busy
            .get_mut(usize::from(slot))
            .ok_or(Error::DeviceFault)?;
        if length != 0 || !*busy {
            return Err(Error::DeviceFault);
        }
        *busy = false;
        Ok(Some(slot))
    }

    /// Copy one received frame and replenish its DMA slot before returning.
    /// Invalid Ethernet lengths are discarded. An empty queue returns None.
    pub fn receive(&mut self, output: &mut [u8]) -> Result<Option<usize>, Error> {
        self.check_running()?;
        if output.len() < self.info.max_frame() {
            return Err(Error::InvalidRequest);
        }
        self.transport.check()?;
        for _ in 0..self.rx_slots {
            let Some((slot, length)) = self.rx.complete()? else {
                return Ok(None);
            };
            if usize::from(slot) >= self.rx_slots || length < HEADER as u32 {
                return Err(Error::DeviceFault);
            }
            let offset = u64::from(slot) * SLOT as u64;
            let mut header = [0; HEADER];
            self.receive.read(offset, &mut header)?;
            // REFERENCE: https://docs.oasis-open.org/virtio/virtio/v1.3/virtio-v1.3.html#x1-2440006
            // No checksum or segmentation offloads were negotiated. num_buffers is unused.
            if header[0] != 0 || header[1] != 0 {
                return Err(Error::DeviceFault);
            }
            let count = length as usize - HEADER;
            let valid = (MIN_FRAME..=self.info.max_frame()).contains(&count);
            if valid {
                self.receive
                    .read(offset + HEADER as u64, &mut output[..count])?;
            }
            self.replenish(slot)?;
            self.rx.notify(&mut self.transport)?;
            if valid {
                return Ok(Some(count));
            }
        }
        Ok(None)
    }

    pub fn acknowledge_interrupt(&mut self) -> Result<(), Error> {
        self.check_running()?;
        if self.transport.interrupt_status()? & 2 != 0 {
            let mut info = configuration_info(&mut self.transport, self.features)?;
            if info.mac != self.info.mac || info.mtu != self.info.mtu {
                return Err(Error::DeviceFault);
            }
            if info.link_up != self.info.link_up {
                info.sequence = self.info.sequence.wrapping_add(1);
                self.info = info;
            }
        }
        Ok(self.interrupt.acknowledge()?)
    }

    pub fn stop(&mut self) -> Result<(), Error> {
        if !self.running {
            return Ok(());
        }
        self.transport.reset()?;
        // SAFETY: Reset has completed, so the device can no longer access this domain's buffers.
        self.domain.quiesce()?;
        self.running = false;
        self.busy.fill(false);
        self.interrupt.unbind()?;
        Ok(())
    }

    fn check_running(&self) -> Result<(), Error> {
        if self.running {
            Ok(())
        } else {
            Err(Error::Unavailable)
        }
    }

    fn replenish(&mut self, slot: u16) -> Result<(), Error> {
        self.rx.publish(
            slot,
            &[Descriptor {
                address: self.receive.bus_address().ok_or(Error::Unavailable)?
                    + u64::from(slot) * SLOT as u64,
                length: SLOT as u32,
                writable: true,
            }],
        )
    }
}

impl Drop for Net<'_> {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn configuration_info(transport: &mut Transport<'_>, features: u64) -> Result<Info, Error> {
    let deadline = thread::now_ns()?.saturating_add(1_000_000_000);
    loop {
        let generation = transport.generation()?;
        let mut mac = [0; 6];
        for (offset, byte) in mac.iter_mut().enumerate() {
            *byte = transport.config_byte(offset)?;
        }
        let link_up = features & STATUS == 0 || transport.config_byte(6)? & 1 != 0;
        let mtu = if features & MTU != 0 {
            u16::from_le_bytes([transport.config_byte(10)?, transport.config_byte(11)?])
        } else {
            1500
        };
        if generation == transport.generation()? {
            if mac[0] & 1 != 0 || mac == [0; 6] || mtu < 68 {
                return Err(Error::InvalidConfiguration);
            }
            return Ok(Info {
                mac,
                mtu: mtu.min(1500),
                link_up,
                sequence: 0,
            });
        }
        if thread::now_ns()? >= deadline {
            return Err(Error::Busy);
        }
        thread::yield_now()?;
    }
}
