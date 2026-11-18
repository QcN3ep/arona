// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Register operations for legacy and modern VirtIO MMIO.

use super::Notification;
use runtime::{Device, Handle, device};

use crate::Error;

pub(crate) struct Mmio<'a> {
    registers: device::Mmio<'a>,
    legacy: bool,
}

impl<'a> Mmio<'a> {
    pub(super) fn open(grant: &'a Handle<Device>, id: u32) -> Result<Self, Error> {
        let mut registers = grant.map_mmio(id, 0)?;
        if registers.len() < 0x100 || registers.read_u32(0)? != 0x7472_6976 {
            return Err(Error::InvalidConfiguration);
        }
        let version = registers.read_u32(4)?;
        if !matches!(version, 1 | 2) || registers.read_u32(8)? != 2 {
            return Err(Error::Unsupported);
        }
        Ok(Self {
            registers,
            legacy: version == 1,
        })
    }

    pub(super) fn legacy(&self) -> bool {
        self.legacy
    }

    pub(super) fn status(&mut self) -> Result<u8, Error> {
        Ok(self.registers.read_u32(0x70)? as u8)
    }

    pub(super) fn set_status(&mut self, value: u8) -> Result<(), Error> {
        Ok(self.registers.write_u32(0x70, value.into())?)
    }

    pub(super) fn features(&mut self) -> Result<u64, Error> {
        self.registers.write_u32(0x14, 0)?;
        let low = self.registers.read_u32(0x10)?;
        if self.legacy {
            return Ok(u64::from(low));
        }
        self.registers.write_u32(0x14, 1)?;
        Ok(u64::from(low) | u64::from(self.registers.read_u32(0x10)?) << 32)
    }

    pub(super) fn accept_features(&mut self, value: u64) -> Result<(), Error> {
        for (select, word) in [(0, value as u32), (1, (value >> 32) as u32)] {
            self.registers.write_u32(0x24, select)?;
            self.registers.write_u32(0x20, word)?;
            if self.legacy {
                break;
            }
        }
        Ok(())
    }

    pub(super) fn queue_size(&mut self, index: u16) -> Result<u16, Error> {
        self.registers.write_u32(0x30, index.into())?;
        u16::try_from(self.registers.read_u32(0x34)?).map_err(|_| Error::InvalidConfiguration)
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
        let configured = self
            .registers
            .read_u32(if self.legacy { 0x40 } else { 0x44 })?;
        if size == 0 || !size.is_power_of_two() || size > maximum || configured != 0 {
            return Err(Error::InvalidConfiguration);
        }
        self.registers.write_u32(0x38, size.into())?;
        if self.legacy {
            let expected_available = descriptor.checked_add(u64::from(size) * 16);
            let expected_used = expected_available
                .and_then(|address| address.checked_add(6 + u64::from(size) * 2))
                .and_then(|address| address.checked_next_multiple_of(4096));
            if !descriptor.is_multiple_of(4096)
                || expected_available != Some(available)
                || expected_used != Some(used)
            {
                return Err(Error::InvalidConfiguration);
            }
            let pfn = u32::try_from(descriptor / 4096).map_err(|_| Error::InvalidConfiguration)?;
            if pfn == 0 {
                return Err(Error::InvalidConfiguration);
            }
            self.registers.write_u32(0x28, 4096)?;
            self.registers.write_u32(0x3c, 4096)?;
            self.registers.write_u32(0x40, pfn)?;
            return Ok(Notification { index, offset: 0 });
        }
        for (offset, address) in [(0x80, descriptor), (0x90, available), (0xa0, used)] {
            self.registers.write_u32(offset, address as u32)?;
            self.registers
                .write_u32(offset + 4, (address >> 32) as u32)?;
        }
        self.registers.write_u32(0x44, 1)?;
        Ok(Notification { index, offset: 0 })
    }

    pub(super) fn notify(&mut self, queue: Notification) -> Result<(), Error> {
        Ok(self.registers.write_u32(0x50, queue.index.into())?)
    }

    pub(super) fn interrupt_status(&mut self) -> Result<u8, Error> {
        let status = self.registers.read_u32(0x60)?;
        self.registers.write_u32(0x64, status)?;
        Ok(status as u8)
    }

    pub(super) fn generation(&mut self) -> Result<u8, Error> {
        if self.legacy {
            return Ok(0);
        }
        Ok(self.registers.read_u32(0xfc)? as u8)
    }

    pub(super) fn config_byte(&mut self, offset: usize) -> Result<u8, Error> {
        if offset >= self.registers.len() - 0x100 {
            return Err(Error::InvalidConfiguration);
        }
        Ok(self.registers.read_u8(0x100 + offset)?)
    }

    pub(super) fn config_word(&mut self, offset: usize) -> Result<u32, Error> {
        if !offset.is_multiple_of(4)
            || offset
                .checked_add(4)
                .is_none_or(|end| end > self.registers.len() - 0x100)
        {
            return Err(Error::InvalidConfiguration);
        }
        Ok(self.registers.read_u32(0x100 + offset)?)
    }
}
