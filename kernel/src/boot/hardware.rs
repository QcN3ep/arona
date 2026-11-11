// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Hardware description of processors, memory, and devices.

mod devices;
mod interrupts;
mod memory;
mod pci;

pub use devices::*;
pub use interrupts::*;
pub use memory::*;
pub use pci::*;

use arrayvec::ArrayVec;

pub const MAX_CPUS: usize = 128;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HardwareInfo {
    pub cpus: ArrayVec<CpuInfo, MAX_CPUS>,
    pub memory: MemoryInfo,
    pub devices: ArrayVec<HardwareDeviceInfo, MAX_DEVICES>,
    pub device_properties: ArrayVec<DeviceProperty, MAX_DEVICE_PROPERTIES>,
    pub timebase_frequency_hz: Option<u64>,
    pub console: Option<ConsoleInfo>,
    pub cpu_interrupts: ArrayVec<CpuInterruptRoute, MAX_CPU_INTERRUPT_ROUTES>,
    pub pci: PciInfo,
}

impl HardwareInfo {
    pub fn device_property(
        &self,
        device: HardwareDeviceInstanceId,
        namespace: &str,
        name: &str,
    ) -> Option<&[u8]> {
        self.device_properties
            .iter()
            .find(|entry| {
                entry.device == device
                    && entry.namespace.as_str() == namespace
                    && entry.name.as_str() == name
            })
            .map(|entry| entry.value.as_slice())
    }

    pub const fn new() -> Self {
        Self {
            cpus: ArrayVec::new_const(),
            memory: MemoryInfo {
                regions: ArrayVec::new_const(),
            },
            devices: ArrayVec::new_const(),
            device_properties: ArrayVec::new_const(),
            timebase_frequency_hz: None,
            console: None,
            cpu_interrupts: ArrayVec::new_const(),
            pci: PciInfo::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuInfo {
    /// Hardware ID provided by the firmware.
    pub hardware_id: u64,
}
