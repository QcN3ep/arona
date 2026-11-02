// SPDX-License-Identifier: MPL-2.0

use crate::{
    hardware,
    memory::{self, DeviceMapping, HeapArc, HeapBox, PhysRange},
};

use super::DeviceError;

const COMMAND: usize = 4;
const IO: u16 = 1;
const MEMORY: u16 = 2;
const MASTER: u16 = 4;
const INTX_DISABLE: u16 = 1 << 10;

pub(in crate::device) struct Configuration {
    mapping: HeapArc<DeviceMapping>,
    offset: usize,
    snapshot: HeapBox<[u8; 4096]>,
    original: [u32; 13],
    pub(in crate::device) bars: [Option<[u8; abi::device::PciBar::SIZE]>; 6],
    pub(in crate::device) unmapped_bars: arrayvec::ArrayVec<PhysRange, 6>,
}

impl Configuration {
    pub(super) fn new(mapping: HeapArc<DeviceMapping>, offset: usize) -> Result<Self, DeviceError> {
        let mut config = Self {
            mapping,
            offset,
            snapshot: memory::manager().allocate_box([0; 4096])?,
            original: [0; 13],
            bars: [None; 6],
            unmapped_bars: arrayvec::ArrayVec::new(),
        };
        config.freeze()?;
        for index in 0..config.original.len() {
            config.original[index] = u32::from_le_bytes(
                config.snapshot[index * 4..index * 4 + 4]
                    .try_into()
                    .unwrap(),
            );
        }
        Ok(config)
    }

    pub(in crate::device) fn snapshot(&self) -> &[u8] {
        self.snapshot.as_slice()
    }

    pub(super) fn original(&self, offset: usize) -> u32 {
        self.original[offset / 4]
    }

    pub(super) fn read(&self, offset: usize) -> Result<u32, DeviceError> {
        hardware::pci_config()
            .read32(&self.mapping, self.offset + offset)
            .map_err(DeviceError::Pci)
    }

    pub(super) fn write(&self, offset: usize, value: u32) -> Result<(), DeviceError> {
        hardware::pci_config()
            .write32(&self.mapping, self.offset + offset, value)
            .map_err(DeviceError::Pci)
    }

    fn read16(&self, offset: usize) -> Result<u16, DeviceError> {
        hardware::pci_config()
            .read16(&self.mapping, self.offset + offset)
            .map_err(DeviceError::Pci)
    }

    fn write16(&self, offset: usize, value: u16) -> Result<(), DeviceError> {
        hardware::pci_config()
            .write16(&self.mapping, self.offset + offset, value)
            .map_err(DeviceError::Pci)
    }

    pub(in crate::device) fn command(
        &self,
        memory: bool,
        interrupts: bool,
    ) -> Result<(), DeviceError> {
        let mut command = self.read16(COMMAND)? & !(IO | MEMORY | MASTER | INTX_DISABLE);
        if memory {
            command |= MEMORY;
        }
        if !interrupts {
            command |= INTX_DISABLE;
        }
        self.write16(COMMAND, command)?;
        if self.read16(COMMAND)? & (IO | MEMORY | MASTER | INTX_DISABLE)
            != command & (IO | MEMORY | MASTER | INTX_DISABLE)
        {
            return Err(DeviceError::InvalidArgument);
        }
        Ok(())
    }

    pub(in crate::device) fn interrupts(&self, enabled: bool) -> Result<(), DeviceError> {
        self.command(self.read16(COMMAND)? & MEMORY != 0, enabled)
    }

    pub(super) fn disable(&self) -> Result<(), DeviceError> {
        self.command(false, false)?;
        let rom = match (self.original(0x0c) >> 16) & 0x7f {
            0 => Some(0x30),
            1 => Some(0x38),
            _ => None,
        };
        if let Some(rom) = rom {
            let value = self.read(rom)? & !1;
            self.write(rom, value)?;
            if self.read(rom)? & 1 != 0 {
                return Err(DeviceError::InvalidArgument);
            }
        }
        self.disable_msi()
    }

    fn disable_msi(&self) -> Result<(), DeviceError> {
        if self.read16(6)? & (1 << 4) == 0 {
            return Ok(());
        }
        let mut pointer = (self.read(0x34)? & 0xff) as usize;
        let mut seen = [false; 64];
        while pointer != 0 {
            if !(0x40..=0xfc).contains(&pointer) || !pointer.is_multiple_of(4) || seen[pointer / 4]
            {
                return Err(DeviceError::InvalidArgument);
            }
            seen[pointer / 4] = true;
            let header = self.read(pointer)?;
            let control = (header >> 16) as u16;
            let (mask, value) = match header & 0xff {
                5 => (1, control & !1),
                0x11 => (0xc000, (control & !0x8000) | 0x4000),
                _ => (0, control),
            };
            if mask != 0 {
                self.write16(pointer + 2, value)?;
                if self.read16(pointer + 2)? & mask != value & mask {
                    return Err(DeviceError::InvalidArgument);
                }
            }
            pointer = ((header >> 8) & 0xff) as usize;
        }
        Ok(())
    }

    pub(super) fn rollback(&self, header_type: u8) -> Result<(), DeviceError> {
        self.rollback_resources(header_type)?;
        if header_type == 1 {
            self.write(0x18, self.original(0x18))?;
            if self.read(0x18)? != self.original(0x18) {
                return Err(DeviceError::InvalidArgument);
            }
        }
        Ok(())
    }

    pub(super) fn rollback_resources(&self, header_type: u8) -> Result<(), DeviceError> {
        self.disable()?;
        if header_type > 1 {
            return Ok(());
        }
        let end = if header_type == 1 { 0x34 } else { 0x28 };
        for offset in (0x10..end).step_by(4) {
            // 0x1c 的高半部是 W1C Secondary Status。
            if header_type == 1 && matches!(offset, 0x18 | 0x1c) {
                continue;
            }
            self.write(offset, self.original(offset))?;
            if self.read(offset)? != self.original(offset) {
                return Err(DeviceError::InvalidArgument);
            }
        }
        Ok(())
    }

    pub(super) fn freeze(&mut self) -> Result<(), DeviceError> {
        for offset in (0..4096).step_by(4) {
            let value = self.read(offset)?;
            self.snapshot[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        Ok(())
    }
    pub(super) fn probe_bars(
        &self,
        header: u8,
    ) -> Result<arrayvec::ArrayVec<crate::PciBar, 6>, DeviceError> {
        let mut bars = arrayvec::ArrayVec::new();
        let count = if header == 1 { 2 } else { 6 };
        let mut index = 0;
        while index < count {
            let offset = 0x10 + usize::from(index) * 4;
            let original = self.original(offset);
            if original & 1 != 0 {
                index += 1;
                continue;
            }
            let width = match (original >> 1) & 3 {
                0 => 32,
                2 if index + 1 < count => 64,
                _ => return Err(DeviceError::InvalidArgument),
            };
            // REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/drivers/pci/probe.c
            let result = (|| {
                self.write(offset, u32::MAX)?;
                if width == 64 {
                    self.write(offset + 4, u32::MAX)?;
                }
                let low = self.read(offset)?;
                let high = if width == 64 {
                    self.read(offset + 4)?
                } else {
                    u32::MAX
                };
                Ok::<_, DeviceError>((low, high))
            })();
            self.write(offset, original)?;
            if width == 64 {
                self.write(offset + 4, self.original(offset + 4))?;
            }
            if self.read(offset)? != original
                || (width == 64 && self.read(offset + 4)? != self.original(offset + 4))
            {
                return Err(DeviceError::InvalidArgument);
            }
            let (low, high) = result?;
            let mask = (u64::from(high) << 32) | u64::from(low & !0xf);
            if low & !0xf != 0 || width == 64 && high != 0 {
                let size = (!mask).checked_add(1).ok_or(DeviceError::InvalidArgument)?;
                if !size.is_power_of_two()
                    || size < 16
                    || mask != !(size - 1)
                    || low & 0xf != original & 0xf
                {
                    return Err(DeviceError::InvalidArgument);
                }
                let bus_address = u64::from(original & !0xf)
                    | if width == 64 {
                        u64::from(self.original(offset + 4)) << 32
                    } else {
                        0
                    };
                bars.push(crate::PciBar {
                    index,
                    width,
                    prefetchable: original & 8 != 0,
                    region: None,
                    bus_address,
                    size,
                });
            }
            index += if width == 64 { 2 } else { 1 };
        }
        Ok(bars)
    }
}
