// SPDX-License-Identifier: MPL-2.0

use kernel::memory::DeviceMapping;

use super::io_fence;

pub(crate) enum MmioError {
    InvalidRange,
}

pub(crate) struct Mmio {
    mapping: DeviceMapping,
}

impl Mmio {
    pub fn new(mapping: DeviceMapping) -> Result<Self, MmioError> {
        if !mapping.physical_range().start.is_multiple_of(4) {
            return Err(MmioError::InvalidRange);
        }
        Ok(Self { mapping })
    }

    pub fn read32(&self, offset: u64) -> Result<u32, MmioError> {
        let address = self.address(offset, 4)?;
        io_fence();
        // SAFETY: The address must be a persistently readable 32-bit device register.
        let value = unsafe { (address as *const u32).read_volatile() };
        io_fence();
        Ok(value)
    }

    pub fn write32(&self, offset: u64, value: u32) -> Result<(), MmioError> {
        let address = self.address(offset, 4)?;
        io_fence();
        // SAFETY: The address must be an exclusively writable 32-bit device register.
        unsafe { (address as *mut u32).write_volatile(value) };
        io_fence();
        Ok(())
    }

    #[cfg(target_arch = "loongarch64")]
    pub fn write8(&self, offset: u64, value: u8) -> Result<(), MmioError> {
        let address = self.address(offset, 1)?;
        io_fence();
        // SAFETY: The address must be an exclusively writable 8-bit device register.
        unsafe { (address as *mut u8).write_volatile(value) };
        io_fence();
        Ok(())
    }

    fn address(&self, offset: u64, width: usize) -> Result<usize, MmioError> {
        let offset = usize::try_from(offset).map_err(|_| MmioError::InvalidRange)?;
        let range = self.mapping.virtual_range();
        let end = offset.checked_add(width).ok_or(MmioError::InvalidRange)?;
        if !offset.is_multiple_of(width) || end > range.end - range.start {
            return Err(MmioError::InvalidRange);
        }
        Ok(range.start + offset)
    }
}
