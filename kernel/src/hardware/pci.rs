// SPDX-License-Identifier: MPL-2.0

//! PCI configuration space access capability.

use crate::memory::DeviceMapping;

use super::{BindError, Slot};

static SLOT: Slot<&'static dyn PciConfigAccess> = Slot::new();

pub fn bind(access: &'static dyn PciConfigAccess) -> Result<(), BindError> {
    SLOT.bind(access)
}

pub fn pci_config() -> &'static dyn PciConfigAccess {
    SLOT.get()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PciConfigError {
    InvalidOffset,
}

pub trait PciConfigAccess: Sync {
    fn read16(&self, mapping: &DeviceMapping, offset: usize) -> Result<u16, PciConfigError>;

    fn write16(
        &self,
        mapping: &DeviceMapping,
        offset: usize,
        value: u16,
    ) -> Result<(), PciConfigError>;

    /// Reads an aligned 32-bit configuration register within the mapping.
    fn read32(&self, mapping: &DeviceMapping, offset: usize) -> Result<u32, PciConfigError>;

    /// Writes an aligned 32-bit configuration register within the mapping.
    fn write32(
        &self,
        mapping: &DeviceMapping,
        offset: usize,
        value: u32,
    ) -> Result<(), PciConfigError>;
}
