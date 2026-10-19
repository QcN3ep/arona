// SPDX-License-Identifier: MPL-2.0

//! 特权内核库。

#![no_std]

pub mod boot;

pub use boot::{
    BootInfo, CpuInfo, DeviceUsePolicy, DmaInfo, DmaIsolationRoute, HardwareDeviceInfo,
    HardwareDeviceInstanceId, HardwareDeviceKind, HardwareDeviceLocation, HardwareInfo,
    HardwareInterruptInfo, InterruptPolarity, InterruptTrigger, MemoryInfo, MemoryRegion,
    MemoryRegionState, PhysRange, boot_kernel,
};
