// SPDX-License-Identifier: MPL-2.0

//! PCI 地址窗口及 INTx、MSI 请求源路由。

use arrayvec::ArrayVec;

use super::{HardwareDeviceInstanceId, HardwareInterruptInfo, PhysRange};

pub const MAX_PCI_INTERRUPT_ROUTES: usize = 64;
pub const MAX_PCI_MSI_MAPS: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PciWindow {
    pub bus_address: u64,
    pub cpu_range: PhysRange,
    pub space: PciAddressSpace,
    pub prefetchable: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PciAddressSpace {
    Memory32,
    Memory64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PciInfo {
    pub interrupts: ArrayVec<PciInterruptRoute, MAX_PCI_INTERRUPT_ROUTES>,
    pub msi_maps: ArrayVec<PciMsiMap, MAX_PCI_MSI_MAPS>,
}

impl PciInfo {
    pub const fn new() -> Self {
        Self {
            interrupts: ArrayVec::new_const(),
            msi_maps: ArrayVec::new_const(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PciInterruptRoute {
    pub host: HardwareDeviceInstanceId,
    pub requester_id: u16,
    pub requester_mask: u16,
    pub pin: u8,
    pub pin_mask: u8,
    pub interrupt: HardwareInterruptInfo,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PciMsiMap {
    pub host: HardwareDeviceInstanceId,
    pub requester_base: u32,
    pub requester_count: u32,
    pub requester_mask: u16,
    pub controller: HardwareDeviceInstanceId,
    pub msi_base: u32,
}
