// SPDX-License-Identifier: MPL-2.0

//! 处理器、内存及设备的硬件描述。

mod devices;
mod interrupts;
mod memory;

pub use devices::*;
pub use interrupts::*;
pub use memory::*;

use arrayvec::ArrayVec;

pub const MAX_CPUS: usize = 128;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HardwareInfo {
    pub cpus: ArrayVec<CpuInfo, MAX_CPUS>,
    pub memory: MemoryInfo,
    pub devices: ArrayVec<HardwareDeviceInfo, MAX_DEVICES>,
    pub timebase_frequency_hz: Option<u64>,
    pub cpu_interrupts: ArrayVec<CpuInterruptRoute, MAX_CPU_INTERRUPT_ROUTES>,
}

impl HardwareInfo {
    pub const fn new() -> Self {
        Self {
            cpus: ArrayVec::new_const(),
            memory: MemoryInfo {
                regions: ArrayVec::new_const(),
            },
            devices: ArrayVec::new_const(),
            timebase_frequency_hz: None,
            cpu_interrupts: ArrayVec::new_const(),
        }
    }

    pub fn validate_resources(&self) -> Result<(), &'static str> {
        if self.cpus.is_empty()
            || self
                .cpus
                .windows(2)
                .any(|pair| pair[0].hardware_id >= pair[1].hardware_id)
        {
            return Err("CPU table is empty or not strictly ordered");
        }
        if self.memory.regions.is_empty()
            || self.memory.regions.iter().any(|region| {
                region.range.start >= region.range.end
                    || !region.range.start.is_multiple_of(4096)
                    || !region.range.end.is_multiple_of(4096)
            })
            || self
                .memory
                .regions
                .windows(2)
                .any(|pair| pair[0].range.end > pair[1].range.start)
        {
            return Err("invalid memory region table");
        }
        if self.timebase_frequency_hz == Some(0) {
            return Err("timebase frequency is zero");
        }
        for (index, device) in self.devices.iter().enumerate() {
            device.validate_configuration()?;
            if let Some(dma) = device.dma {
                if !(1..=64).contains(&dma.address_bits) {
                    return Err("invalid DMA address width");
                }
                if let Some(route) = dma.isolation
                    && self.device(route.iommu)?.kind != HardwareDeviceKind::IOMMU
                {
                    return Err("invalid DMA isolation controller");
                }
            }
            if device.instance_id.0 == 0
                || self.devices[..index]
                    .iter()
                    .any(|other| other.instance_id == device.instance_id)
            {
                return Err("invalid or duplicate device instance ID");
            }
            self.validate_addresses(index, device)?;
            if let HardwareDeviceLocation::Iocsr { registers } = &device.location
                && (registers.is_empty() || device.use_policy != DeviceUsePolicy::KernelReserved)
            {
                return Err("invalid IOCSR resource");
            }
            if self.devices[..index]
                .iter()
                .any(|other| device.location.overlaps(&other.location))
            {
                return Err("device locations overlap");
            }
        }
        self.validate_interrupts()?;
        Ok(())
    }

    pub(super) fn device(
        &self,
        id: HardwareDeviceInstanceId,
    ) -> Result<&HardwareDeviceInfo, &'static str> {
        self.devices
            .iter()
            .find(|device| device.instance_id == id)
            .ok_or("referenced device is missing from the device table")
    }

    fn validate_addresses(
        &self,
        index: usize,
        device: &HardwareDeviceInfo,
    ) -> Result<(), &'static str> {
        for (position, address) in device.addresses().enumerate() {
            let pages = address.page_envelope()?;
            if self
                .memory
                .regions
                .iter()
                .any(|region| region.range.overlaps(pages))
            {
                return Err("device resource envelope overlaps RAM");
            }
            if device
                .addresses()
                .take(position)
                .any(|other| address.overlaps(other))
            {
                return Err("device resources overlap within the same device");
            }
            for other in &self.devices[..index] {
                for other_address in other.addresses() {
                    if address.overlaps(other_address) {
                        return Err("device register ranges overlap");
                    }
                    if (device.use_policy == DeviceUsePolicy::UserAssignable
                        || other.use_policy == DeviceUsePolicy::UserAssignable)
                        && pages.overlaps(other_address.page_envelope()?)
                    {
                        return Err("assignable device shares a resource page");
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuInfo {
    /// 固件提供的硬件 ID。
    pub hardware_id: u64,
}
