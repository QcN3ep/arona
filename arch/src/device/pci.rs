// SPDX-License-Identifier: MPL-2.0

//! ECAM 配置空间访问。

use kernel::{
    hardware::pci::{PciConfigAccess, PciConfigError},
    memory::DeviceMapping,
};

use super::io_fence;

crate::declare_hardware!(
    static PCI_CONFIG: PciConfig = PciConfig => kernel::hardware::pci::bind
);

struct PciConfig;

impl PciConfigAccess for PciConfig {
    fn read16(&self, mapping: &DeviceMapping, offset: usize) -> Result<u16, PciConfigError> {
        let address = address(mapping, offset, 2)?;
        io_fence();
        // SAFETY: mapping 须覆盖 ECAM 中对齐且可读的 16 位配置项。
        let value = unsafe { (address as *const u16).read_volatile() };
        io_fence();
        Ok(u16::from_le(value))
    }

    fn write16(
        &self,
        mapping: &DeviceMapping,
        offset: usize,
        value: u16,
    ) -> Result<(), PciConfigError> {
        let address = address(mapping, offset, 2)?;
        io_fence();
        // SAFETY: mapping 须覆盖 ECAM 中对齐且可写的 16 位配置项。
        unsafe { (address as *mut u16).write_volatile(value.to_le()) };
        io_fence();
        Ok(())
    }

    fn read32(&self, mapping: &DeviceMapping, offset: usize) -> Result<u32, PciConfigError> {
        let address = address(mapping, offset, 4)?;
        io_fence();
        // SAFETY: mapping 须覆盖 ECAM 中对齐且可读的 32 位配置项。
        let value = unsafe { (address as *const u32).read_volatile() };
        io_fence();
        Ok(u32::from_le(value))
    }

    fn write32(
        &self,
        mapping: &DeviceMapping,
        offset: usize,
        value: u32,
    ) -> Result<(), PciConfigError> {
        let address = address(mapping, offset, 4)?;
        io_fence();
        // SAFETY: mapping 须覆盖 ECAM 中对齐且可写的 32 位配置项。
        unsafe { (address as *mut u32).write_volatile(value.to_le()) };
        io_fence();
        Ok(())
    }
}

fn address(mapping: &DeviceMapping, offset: usize, width: usize) -> Result<usize, PciConfigError> {
    let range = mapping.virtual_range();
    let length = range
        .end
        .checked_sub(range.start)
        .ok_or(PciConfigError::InvalidOffset)?;
    let end = offset
        .checked_add(width)
        .ok_or(PciConfigError::InvalidOffset)?;
    if !offset.is_multiple_of(width) || end > length {
        return Err(PciConfigError::InvalidOffset);
    }
    range
        .start
        .checked_add(offset)
        .ok_or(PciConfigError::InvalidOffset)
}
