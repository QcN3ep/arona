// SPDX-License-Identifier: MPL-2.0

//! Register operations for VirtIO MMIO version 2.

use runtime::{Device, Handle, device};

use crate::Error;

pub(crate) struct Mmio<'a>(device::Mmio<'a>);

impl<'a> Mmio<'a> {
    pub(super) fn open(grant: &'a Handle<Device>, id: u32) -> Result<Self, Error> {
        let mut registers = grant.map_mmio(id, 0)?;
        if registers.len() < 0x118 || registers.read_u32(0)? != 0x7472_6976 {
            return Err(Error::InvalidConfiguration);
        }
        if registers.read_u32(4)? != 2 || registers.read_u32(8)? != 2 {
            return Err(Error::Unsupported);
        }
        Ok(Self(registers))
    }

    pub(super) fn status(&mut self) -> Result<u8, Error> {
        Ok(self.0.read_u32(0x70)? as u8)
    }

    pub(super) fn set_status(&mut self, value: u8) -> Result<(), Error> {
        Ok(self.0.write_u32(0x70, value.into())?)
    }

    pub(super) fn features(&mut self) -> Result<u64, Error> {
        self.0.write_u32(0x14, 0)?;
        let low = self.0.read_u32(0x10)?;
        self.0.write_u32(0x14, 1)?;
        Ok(u64::from(low) | u64::from(self.0.read_u32(0x10)?) << 32)
    }

    pub(super) fn accept_features(&mut self, value: u64) -> Result<(), Error> {
        for (select, word) in [(0, value as u32), (1, (value >> 32) as u32)] {
            self.0.write_u32(0x24, select)?;
            self.0.write_u32(0x20, word)?;
        }
        Ok(())
    }

    pub(super) fn queue_size(&mut self) -> Result<u16, Error> {
        self.0.write_u32(0x30, 0)?;
        u16::try_from(self.0.read_u32(0x34)?).map_err(|_| Error::InvalidConfiguration)
    }

    pub(super) fn configure_queue(
        &mut self,
        size: u16,
        descriptor: u64,
        available: u64,
        used: u64,
    ) -> Result<(), Error> {
        if self.0.read_u32(0x44)? != 0 {
            return Err(Error::InvalidConfiguration);
        }
        self.0.write_u32(0x38, size.into())?;
        for (offset, address) in [(0x80, descriptor), (0x90, available), (0xa0, used)] {
            self.0.write_u32(offset, address as u32)?;
            self.0.write_u32(offset + 4, (address >> 32) as u32)?;
        }
        Ok(self.0.write_u32(0x44, 1)?)
    }

    pub(super) fn notify(&mut self) -> Result<(), Error> {
        Ok(self.0.write_u32(0x50, 0)?)
    }

    pub(super) fn interrupt_status(&mut self) -> Result<u8, Error> {
        let status = self.0.read_u32(0x60)?;
        self.0.write_u32(0x64, status)?;
        Ok(status as u8)
    }

    pub(super) fn generation(&mut self) -> Result<u8, Error> {
        Ok(self.0.read_u32(0xfc)? as u8)
    }

    pub(super) fn config_word(&mut self, offset: usize) -> Result<u32, Error> {
        Ok(self.0.read_u32(0x100 + offset)?)
    }
}
