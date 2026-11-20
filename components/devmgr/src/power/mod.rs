// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Platform power capability discovery and system controller register writes.

pub mod service;
mod session;

pub use session::Session;

use crate::{
    Error,
    device::{ByteOrder, DeviceDescription},
};
use runtime::{Device, Handle, device::Mmio};

const NAMESPACE: &str = "power";
const POWEROFF: &str = "poweroff";
const REBOOT: &str = "reboot";
const REGISTER_WIDTH: &str = "register-width";
const DEFAULT_REGISTER_WIDTH: u8 = 4;
const OPERATION_BYTES: usize = 12;

/// A system controller update applied as `(old & !mask) | (value & mask)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RegisterWrite {
    offset: u32,
    mask: u32,
    value: u32,
}

/// Power actions supported by a system controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Capabilities {
    pub shutdown: bool,
    pub reboot: bool,
}

/// A probed system controller and the power actions it provides.
#[derive(Clone, Copy, Debug)]
pub struct Configuration {
    id: u32,
    register_width: u8,
    byte_order: ByteOrder,
    poweroff: Option<RegisterWrite>,
    reboot: Option<RegisterWrite>,
}

impl Configuration {
    pub fn capabilities(self) -> Capabilities {
        Capabilities {
            shutdown: self.poweroff.is_some(),
            reboot: self.reboot.is_some(),
        }
    }

    fn register(&self, write: RegisterWrite) -> Result<Register, Error> {
        let bits = match self.register_width {
            1 => 0xff,
            2 => 0xffff,
            _ => u32::MAX,
        };
        if write.mask == 0
            || write.mask & !bits != 0
            || write.value & !bits != 0
            || !write.offset.is_multiple_of(u32::from(self.register_width))
        {
            return Err(Error::InvalidConfiguration);
        }
        Ok(Register {
            offset: write.offset,
            mask: write.mask,
            value: write.value,
        })
    }
}

struct Register {
    offset: u32,
    mask: u32,
    value: u32,
}

/// A mapped system controller that performs power operations on its registers.
pub struct Power<'a> {
    registers: Mmio<'a>,
    configuration: Configuration,
}

/// Match a system controller that publishes power operations.
pub fn probe(device: &DeviceDescription<'_>) -> Result<Option<Configuration>, Error> {
    if !device.matches("devicetree", "syscon")? {
        return Ok(None);
    }
    let mut bytes = [0; OPERATION_BYTES];
    let poweroff = operation(device, POWEROFF, &mut bytes)?;
    let reboot = operation(device, REBOOT, &mut bytes)?;
    if poweroff.is_none() && reboot.is_none() {
        return Ok(None);
    }
    if device.info().mmio_regions != 1 {
        return Err(Error::InvalidConfiguration);
    }
    let register_width = match device.cell(NAMESPACE, REGISTER_WIDTH)? {
        Some(width @ (1 | 2 | 4)) => width as u8,
        Some(_) => return Err(Error::InvalidConfiguration),
        None => DEFAULT_REGISTER_WIDTH,
    };
    Ok(Some(Configuration {
        id: device.info().id,
        register_width,
        byte_order: device.byte_order(NAMESPACE)?,
        poweroff,
        reboot,
    }))
}

fn operation(
    device: &DeviceDescription<'_>,
    name: &str,
    bytes: &mut [u8; OPERATION_BYTES],
) -> Result<Option<RegisterWrite>, Error> {
    let Some(value) = device.property(NAMESPACE, name, bytes)? else {
        return Ok(None);
    };
    let value: [u8; OPERATION_BYTES] = value.try_into().map_err(|_| Error::InvalidConfiguration)?;
    Ok(Some(RegisterWrite {
        offset: u32::from_be_bytes([value[0], value[1], value[2], value[3]]),
        mask: u32::from_be_bytes([value[4], value[5], value[6], value[7]]),
        value: u32::from_be_bytes([value[8], value[9], value[10], value[11]]),
    }))
}

impl<'a> Power<'a> {
    pub fn new(configuration: Configuration, grant: &'a Handle<Device>) -> Result<Self, Error> {
        let mut length = 0;
        for write in [configuration.poweroff, configuration.reboot]
            .into_iter()
            .flatten()
        {
            configuration.register(write)?;
            length = length.max(write.offset as usize + usize::from(configuration.register_width));
        }
        let registers = grant.map_mmio(configuration.id, 0)?;
        if registers.len() < length {
            return Err(Error::InvalidConfiguration);
        }
        Ok(Self {
            registers,
            configuration,
        })
    }

    pub fn capabilities(&self) -> Capabilities {
        self.configuration.capabilities()
    }

    pub fn shutdown(&mut self) -> Result<(), Error> {
        let write = self.configuration.poweroff.ok_or(Error::Unsupported)?;
        self.perform(self.configuration.register(write)?)
    }

    pub fn reboot(&mut self) -> Result<(), Error> {
        let write = self.configuration.reboot.ok_or(Error::Unsupported)?;
        self.perform(self.configuration.register(write)?)
    }

    fn perform(&mut self, register: Register) -> Result<(), Error> {
        let offset = usize::try_from(register.offset).map_err(|_| Error::InvalidConfiguration)?;
        let current = self.read(offset)?;
        let value = (current & !register.mask) | (register.value & register.mask);
        self.write(offset, value)
    }

    fn read(&mut self, offset: usize) -> Result<u32, Error> {
        self.configuration.byte_order.read(
            &mut self.registers,
            offset,
            usize::from(self.configuration.register_width),
        )
    }

    fn write(&mut self, offset: usize, value: u32) -> Result<(), Error> {
        self.configuration.byte_order.write(
            &mut self.registers,
            offset,
            usize::from(self.configuration.register_width),
            value,
        )
    }
}
