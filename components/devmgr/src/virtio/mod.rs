// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Negotiation, queue configuration, and reset for the VirtIO modern transport.

mod mmio;
mod pci;
pub(crate) mod queue;

use alloc::boxed::Box;

use runtime::{Device, Handle, thread};

use crate::{Error, device::DeviceDescription};

// REFERENCE: https://docs.oasis-open.org/virtio/virtio/v1.3/virtio-v1.3.html
const VERSION_1: u64 = 1 << 32;
const ACKNOWLEDGE: u8 = 1;
const DRIVER: u8 = 2;
const DRIVER_OK: u8 = 4;
const FEATURES_OK: u8 = 8;
const NEEDS_RESET: u8 = 64;
const FAILED: u8 = 128;
const RESET_TIMEOUT_NS: u64 = 1_000_000_000;
pub(crate) const BLOCK_DEVICE_TYPE: u16 = 2;
pub(crate) const RNG_DEVICE_TYPE: u16 = 4;

#[derive(Clone, Copy, Debug)]
pub(crate) enum Configuration {
    Mmio(u32),
    Pci(pci::Configuration),
}

pub(crate) enum Transport<'a> {
    Mmio(mmio::Mmio<'a>),
    Pci(Box<pci::Pci<'a>>),
}

pub(crate) fn probe(
    device: &DeviceDescription<'_>,
    device_type: u16,
) -> Result<Option<Configuration>, Error> {
    let identity = match device_type {
        BLOCK_DEVICE_TYPE => "1af4:1042",
        RNG_DEVICE_TYPE => "1af4:1044",
        _ => return Err(Error::Unsupported),
    };
    if device.matches("pci", identity)? {
        return Ok(Some(Configuration::Pci(pci::probe(device, device_type)?)));
    }
    if device_type == BLOCK_DEVICE_TYPE && device.matches("devicetree", "virtio,mmio")? {
        if device.info().mmio_regions != 1 || device.info().interrupts != 1 {
            return Err(Error::Unsupported);
        }
        return Ok(Some(Configuration::Mmio(device.info().id)));
    }
    Ok(None)
}

macro_rules! transport {
    ($name:ident($($argument:ident: $ty:ty),*) -> $result:ty) => {
        pub(crate) fn $name(&mut self, $($argument: $ty),*) -> Result<$result, Error> {
            match self {
                Self::Mmio(registers) => registers.$name($($argument),*),
                Self::Pci(registers) => registers.$name($($argument),*),
            }
        }
    };
}

impl<'a> Transport<'a> {
    pub(crate) fn open(config: Configuration, grant: &'a Handle<Device>) -> Result<Self, Error> {
        match config {
            Configuration::Mmio(id) => Ok(Self::Mmio(mmio::Mmio::open(grant, id)?)),
            Configuration::Pci(config) => Ok(Self::Pci(Box::new(pci::Pci::open(grant, config)?))),
        }
    }

    pub(crate) fn negotiate(&mut self, supported: u64) -> Result<u64, Error> {
        self.reset()?;
        self.set_status(ACKNOWLEDGE)?;
        self.set_status(ACKNOWLEDGE | DRIVER)?;
        let offered = self.features()?;
        if offered & VERSION_1 == 0 {
            return Err(Error::Unsupported);
        }
        let accepted = offered & supported | VERSION_1;
        self.accept_features(accepted)?;
        self.set_status(ACKNOWLEDGE | DRIVER | FEATURES_OK)?;
        if self.status()? != ACKNOWLEDGE | DRIVER | FEATURES_OK {
            return Err(Error::Unsupported);
        }
        Ok(accepted)
    }

    pub(crate) fn start(&mut self) -> Result<(), Error> {
        self.set_status(ACKNOWLEDGE | DRIVER | FEATURES_OK | DRIVER_OK)?;
        self.check()
    }

    pub(crate) fn check(&mut self) -> Result<(), Error> {
        let status = self.status()?;
        if status & (NEEDS_RESET | FAILED) != 0 || status & DRIVER_OK == 0 {
            return Err(Error::DeviceFault);
        }
        Ok(())
    }

    pub(crate) fn reset(&mut self) -> Result<(), Error> {
        self.set_status(0)?;
        let deadline = thread::now_ns()?.saturating_add(RESET_TIMEOUT_NS);
        while self.status()? != 0 {
            if thread::now_ns()? >= deadline {
                return Err(Error::DeviceFault);
            }
            thread::yield_now()?;
        }
        Ok(())
    }

    transport!(status() -> u8);
    transport!(set_status(value: u8) -> ());
    transport!(features() -> u64);
    transport!(accept_features(value: u64) -> ());
    transport!(queue_size() -> u16);
    transport!(configure_queue(size: u16, descriptor: u64, available: u64, used: u64) -> ());
    transport!(notify() -> ());
    transport!(interrupt_status() -> u8);
    transport!(generation() -> u8);
    transport!(config_word(offset: usize) -> u32);
}
