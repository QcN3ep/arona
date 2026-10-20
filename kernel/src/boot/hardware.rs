// SPDX-License-Identifier: MPL-2.0

//! 处理器、内存及设备的硬件描述。

mod devices;
mod memory;

pub use devices::*;
pub use memory::*;

use arrayvec::ArrayVec;

pub const MAX_CPUS: usize = 128;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HardwareInfo {
    pub cpus: ArrayVec<CpuInfo, MAX_CPUS>,
    pub memory: MemoryInfo,
    pub devices: ArrayVec<HardwareDeviceInfo, MAX_DEVICES>,
}

impl HardwareInfo {
    pub const fn new() -> Self {
        Self {
            cpus: ArrayVec::new_const(),
            memory: MemoryInfo {
                regions: ArrayVec::new_const(),
            },
            devices: ArrayVec::new_const(),
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
        for (index, device) in self.devices.iter().enumerate() {
            for (position, address) in device.addresses().enumerate() {
                if self
                    .memory
                    .regions
                    .iter()
                    .any(|region| region.range.overlaps(address))
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
                if self.devices[..index]
                    .iter()
                    .flat_map(HardwareDeviceInfo::addresses)
                    .any(|other| address.overlaps(other))
                {
                    return Err("device resource envelopes overlap");
                }
            }
            for (position, interrupt) in device.interrupts.iter().enumerate() {
                let duplicate = self.devices[..index]
                    .iter()
                    .flat_map(|device| &device.interrupts)
                    .chain(&device.interrupts[..position])
                    .any(|other| {
                        other.controller == interrupt.controller && other.line == interrupt.line
                    });
                if duplicate {
                    return Err("shared IRQs are not supported");
                }
            }
            if self.devices[..index]
                .iter()
                .any(|other| device.location.bus_range_overlaps(&other.location))
            {
                return Err("PCI host bridge bus ranges overlap");
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
