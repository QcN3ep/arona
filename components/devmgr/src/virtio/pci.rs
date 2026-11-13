// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! VirtIO PCI capability validation and the modern memory transport.

use runtime::{Device, Handle, device::Mmio};

use super::Notification;
use crate::{Error, device::DeviceDescription};

const VIRTIO_DEVICE_ID_BASE: u16 = 0x1040;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Configuration {
    id: u32,
    common: Region,
    notify: Region,
    isr: Region,
    device: Option<Region>,
    multiplier: usize,
}

#[derive(Clone, Copy, Debug)]
struct Region {
    index: usize,
    offset: usize,
    length: usize,
}

pub(crate) struct Pci<'a> {
    common: Mmio<'a>,
    notify: Mmio<'a>,
    isr: Mmio<'a>,
    device: Option<Mmio<'a>>,
    config: Configuration,
}

// REFERENCE: https://docs.oasis-open.org/virtio/virtio/v1.3/virtio-v1.3.html#x1-1100004
pub(super) fn probe(
    device: &DeviceDescription<'_>,
    device_type: u16,
) -> Result<Configuration, Error> {
    if device.info().interrupts != 1 {
        return Err(Error::Unsupported);
    }
    let mut bytes = [0; 4096];
    device.pci_config(&mut bytes)?;
    if bytes[..2] != [0xf4, 0x1a] || bytes[6] & 0x10 == 0 {
        return Err(Error::InvalidConfiguration);
    }
    let device_id = u16::from_le_bytes([bytes[2], bytes[3]]);
    // Transitional devices keep a legacy PCI ID and report the VirtIO device
    // type through the subsystem device ID, which separates them from
    // unrelated devices that reuse the same legacy ID.
    let subsystem_id = u16::from_le_bytes([bytes[0x2e], bytes[0x2f]]);
    let modern = device_id == VIRTIO_DEVICE_ID_BASE + device_type;
    let transitional = legacy_device_id(device_type)
        .is_some_and(|legacy| device_id == legacy && subsystem_id == device_type);
    if !modern && !transitional {
        return Err(Error::InvalidConfiguration);
    }
    let mut regions = [None; 4];
    let mut multiplier = 0;
    let mut seen = [false; 64];
    let mut offset = usize::from(bytes[0x34]);
    while offset != 0 {
        if !(0x40..=0xfc).contains(&offset) || !offset.is_multiple_of(4) || seen[offset / 4] {
            return Err(Error::InvalidConfiguration);
        }
        seen[offset / 4] = true;
        if bytes[offset] == 9 {
            let length = usize::from(bytes[offset + 2]);
            if length < 16 || offset + length > 256 {
                return Err(Error::InvalidConfiguration);
            }
            let kind = bytes[offset + 3];
            if (1..=4).contains(&kind) {
                let index = usize::from(kind - 1);
                if regions[index].is_some() || kind == 2 && length < 20 {
                    return Err(Error::InvalidConfiguration);
                }
                let start = word(&bytes, offset + 8) as usize;
                let size = word(&bytes, offset + 12) as usize;
                // Devices without configuration fields can advertise an empty region.
                if kind == 4 && size == 0 {
                    offset = usize::from(bytes[offset + 1]);
                    continue;
                }
                let bar = device.pci_bar(usize::from(bytes[offset + 4]))?;
                let minimum = [56, 2, 1, 1][index];
                let alignment = [4, 2, 1, 1][index];
                if size < minimum
                    || !start.is_multiple_of(alignment)
                    || start
                        .checked_add(size)
                        .is_none_or(|end| end as u64 > bar.size)
                {
                    return Err(Error::InvalidConfiguration);
                }
                regions[index] = Some(Region {
                    index: usize::from(bar.region.ok_or(Error::Unsupported)?),
                    offset: start,
                    length: size,
                });
                if kind == 2 {
                    multiplier = word(&bytes, offset + 16) as usize;
                    if !multiplier.is_multiple_of(2) {
                        return Err(Error::InvalidConfiguration);
                    }
                }
            }
        }
        offset = usize::from(bytes[offset + 1]);
    }
    if regions.iter().all(Option::is_none) {
        // A legacy-only device exposes no modern capability regions. Its
        // register block lives in an I/O BAR, and it does not offer the
        // VIRTIO_F_VERSION_1 feature the transport negotiates.
        return Err(Error::Unsupported);
    }
    let [Some(common), Some(notify), Some(isr), config] = regions else {
        return Err(Error::InvalidConfiguration);
    };
    Ok(Configuration {
        id: device.info().id,
        common,
        notify,
        isr,
        device: config,
        multiplier,
    })
}

impl<'a> Pci<'a> {
    pub(super) fn open(grant: &'a Handle<Device>, config: Configuration) -> Result<Self, Error> {
        Ok(Self {
            common: grant.map_mmio(config.id, config.common.index)?,
            notify: grant.map_mmio(config.id, config.notify.index)?,
            isr: grant.map_mmio(config.id, config.isr.index)?,
            device: config
                .device
                .map(|region| grant.map_mmio(config.id, region.index))
                .transpose()?,
            config,
        })
    }

    pub(super) fn status(&mut self) -> Result<u8, Error> {
        Ok(self.common.read_u8(self.config.common.offset + 20)?)
    }

    pub(super) fn set_status(&mut self, value: u8) -> Result<(), Error> {
        Ok(self
            .common
            .write_u8(self.config.common.offset + 20, value)?)
    }

    pub(super) fn features(&mut self) -> Result<u64, Error> {
        let offset = self.config.common.offset;
        self.common.write_u32(offset, 0)?;
        let low = self.common.read_u32(offset + 4)?;
        self.common.write_u32(offset, 1)?;
        Ok(u64::from(low) | u64::from(self.common.read_u32(offset + 4)?) << 32)
    }

    pub(super) fn accept_features(&mut self, value: u64) -> Result<(), Error> {
        let offset = self.config.common.offset;
        for (select, word) in [(0, value as u32), (1, (value >> 32) as u32)] {
            self.common.write_u32(offset + 8, select)?;
            self.common.write_u32(offset + 12, word)?;
        }
        Ok(())
    }

    fn select_queue(&mut self, index: u16) -> Result<(), Error> {
        let offset = self.config.common.offset;
        if index >= self.common.read_u16(offset + 18)? {
            return Err(Error::InvalidConfiguration);
        }
        Ok(self.common.write_u16(offset + 22, index)?)
    }

    pub(super) fn queue_size(&mut self, index: u16) -> Result<u16, Error> {
        self.select_queue(index)?;
        Ok(self.common.read_u16(self.config.common.offset + 24)?)
    }

    pub(super) fn configure_queue(
        &mut self,
        index: u16,
        size: u16,
        descriptor: u64,
        available: u64,
        used: u64,
    ) -> Result<Notification, Error> {
        let maximum = self.queue_size(index)?;
        let offset = self.config.common.offset;
        if size == 0
            || !size.is_power_of_two()
            || size > maximum
            || self.common.read_u16(offset + 28)? != 0
        {
            return Err(Error::InvalidConfiguration);
        }
        let notify = usize::from(self.common.read_u16(offset + 30)?);
        let notify = notify
            .checked_mul(self.config.multiplier)
            .filter(|start| {
                start
                    .checked_add(2)
                    .is_some_and(|end| end <= self.config.notify.length)
            })
            .ok_or(Error::InvalidConfiguration)?;
        self.common.write_u16(offset + 24, size)?;
        for (field, address) in [(32, descriptor), (40, available), (48, used)] {
            self.common.write_u32(offset + field, address as u32)?;
            self.common
                .write_u32(offset + field + 4, (address >> 32) as u32)?;
        }
        self.common.write_u16(offset + 28, 1)?;
        Ok(Notification {
            index,
            offset: self.config.notify.offset + notify,
        })
    }

    pub(super) fn notify(&mut self, queue: Notification) -> Result<(), Error> {
        Ok(self.notify.write_u16(queue.offset, queue.index)?)
    }

    pub(super) fn interrupt_status(&mut self) -> Result<u8, Error> {
        Ok(self.isr.read_u8(self.config.isr.offset)?)
    }

    pub(super) fn generation(&mut self) -> Result<u8, Error> {
        Ok(self.common.read_u8(self.config.common.offset + 21)?)
    }

    pub(super) fn config_byte(&mut self, offset: usize) -> Result<u8, Error> {
        let region = self.config.device.ok_or(Error::InvalidConfiguration)?;
        if offset >= region.length {
            return Err(Error::InvalidConfiguration);
        }
        let device = self.device.as_mut().ok_or(Error::InvalidConfiguration)?;
        Ok(device.read_u8(region.offset + offset)?)
    }

    pub(super) fn config_word(&mut self, offset: usize) -> Result<u32, Error> {
        let region = self.config.device.ok_or(Error::InvalidConfiguration)?;
        if offset.checked_add(4).is_none_or(|end| end > region.length)
            || !(region.offset + offset).is_multiple_of(4)
        {
            return Err(Error::InvalidConfiguration);
        }
        let device = self.device.as_mut().ok_or(Error::InvalidConfiguration)?;
        Ok(device.read_u32(region.offset + offset)?)
    }
}

fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn legacy_device_id(device_type: u16) -> Option<u16> {
    match device_type {
        super::NET_DEVICE_TYPE => Some(0x1000),
        super::BLOCK_DEVICE_TYPE => Some(0x1001),
        super::RNG_DEVICE_TYPE => Some(0x1005),
        _ => None,
    }
}
