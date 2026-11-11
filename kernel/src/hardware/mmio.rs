// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use crate::{
    HardwareDeviceInstanceId, HardwareInfo,
    memory::{DeviceMapping, MemoryError},
};

pub trait MmioMapper {
    fn map(
        &self,
        device: HardwareDeviceInstanceId,
        region: usize,
    ) -> Result<DeviceMapping, MemoryError>;
}

pub struct DeviceMapper<'a> {
    hardware: &'a HardwareInfo,
}

impl<'a> DeviceMapper<'a> {
    pub fn new(hardware: &'a HardwareInfo) -> Self {
        Self { hardware }
    }
}

impl MmioMapper for DeviceMapper<'_> {
    fn map(
        &self,
        device: HardwareDeviceInstanceId,
        region: usize,
    ) -> Result<DeviceMapping, MemoryError> {
        let physical = self
            .hardware
            .devices
            .iter()
            .find(|info| info.instance_id == device)
            .and_then(|info| info.mmio_regions.get(region))
            .copied()
            .ok_or(MemoryError::InvalidRequest)?;
        DeviceMapping::new(physical)
    }
}
