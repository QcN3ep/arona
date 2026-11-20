// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! ECAM configuration space access.

use kernel::{
    hardware::pci::{PciConfigAccess, PciConfigError},
    memory::DeviceMapping,
};

use super::mmio::{Mmio, MmioError};

crate::declare_hardware!(
    static PCI_CONFIG: PciConfig = PciConfig => kernel::hardware::pci::bind
);

struct PciConfig;

impl From<MmioError> for PciConfigError {
    fn from(_: MmioError) -> Self {
        Self::InvalidOffset
    }
}

impl PciConfigAccess for PciConfig {
    fn read16(&self, mapping: &DeviceMapping, offset: usize) -> Result<u16, PciConfigError> {
        Ok(u16::from_le(Mmio::new(mapping)?.read16(offset as u64)?))
    }
    fn write16(
        &self,
        mapping: &DeviceMapping,
        offset: usize,
        value: u16,
    ) -> Result<(), PciConfigError> {
        Ok(Mmio::new(mapping)?.write16(offset as u64, value.to_le())?)
    }
    fn read32(&self, mapping: &DeviceMapping, offset: usize) -> Result<u32, PciConfigError> {
        Ok(u32::from_le(Mmio::new(mapping)?.read32(offset as u64)?))
    }
    fn write32(
        &self,
        mapping: &DeviceMapping,
        offset: usize,
        value: u32,
    ) -> Result<(), PciConfigError> {
        Ok(Mmio::new(mapping)?.write32(offset as u64, value.to_le())?)
    }
}
