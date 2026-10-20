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

impl PciInfo {
    pub(super) fn validate(&self, hardware: &super::HardwareInfo) -> Result<(), &'static str> {
        use super::{DeviceConfiguration, HardwareDeviceKind};
        self.validate_hosts(hardware)?;
        for (index, route) in self.interrupts.iter().enumerate() {
            if hardware.device(route.host)?.kind != HardwareDeviceKind::PCI_HOST_BRIDGE
                || !(1..=4).contains(&route.pin)
                || route.pin_mask > 7
                || route.requester_id & !route.requester_mask != 0
                || route.pin & !route.pin_mask != 0
            {
                return Err("invalid PCI interrupt route");
            }
            let source = hardware.interrupt_source(route.interrupt)?;
            for interrupt in hardware
                .devices
                .iter()
                .flat_map(|device| &device.interrupts)
            {
                if hardware.interrupt_source(*interrupt)? == source {
                    return Err("PCI interrupt overlaps a platform interrupt");
                }
            }
            if self.interrupts[..index].iter().any(|other| {
                route.host == other.host
                    && (route.requester_id ^ other.requester_id)
                        & route.requester_mask
                        & other.requester_mask
                        == 0
                    && (route.pin ^ other.pin) & route.pin_mask & other.pin_mask == 0
            }) {
                return Err("ambiguous PCI interrupt route");
            }
        }
        for (index, map) in self.msi_maps.iter().enumerate() {
            if hardware.device(map.host)?.kind != HardwareDeviceKind::PCI_HOST_BRIDGE {
                return Err("invalid PCI MSI host");
            }
            let controller = hardware.device(map.controller)?;
            if !matches!(controller.configuration, DeviceConfiguration::Msi { .. })
                || map.requester_count == 0
                || map.requester_base & !u32::from(map.requester_mask) != 0
                || map
                    .requester_base
                    .checked_add(map.requester_count)
                    .is_none_or(|end| end > 65536)
                || map.msi_base.checked_add(map.requester_count).is_none()
            {
                return Err("invalid PCI MSI requester map");
            }
            if self.msi_maps[..index].iter().any(|other| {
                map.host == other.host
                    && (map.requester_mask != other.requester_mask
                        || map.requester_base < other.requester_base + other.requester_count
                            && other.requester_base < map.requester_base + map.requester_count)
            }) {
                return Err("ambiguous PCI MSI requester map");
            }
        }
        Ok(())
    }
    fn validate_hosts(&self, hardware: &super::HardwareInfo) -> Result<(), &'static str> {
        use super::{HardwareDeviceKind, HardwareDeviceLocation};
        for host in &hardware.devices {
            if let HardwareDeviceLocation::PciHostBridge {
                first_bus,
                last_bus,
                ecam,
                windows,
                iommu,
                ..
            } = &host.location
            {
                if first_bus > last_bus
                    || !ecam.start.is_multiple_of(4096)
                    || !ecam.end.is_multiple_of(4096)
                    || ecam.end.saturating_sub(ecam.start)
                        < (u64::from(last_bus - first_bus) + 1) << 20
                {
                    return Err("invalid PCI ECAM bus range");
                }
                if let Some(id) = iommu
                    && hardware.device(*id)?.kind != HardwareDeviceKind::IOMMU
                {
                    return Err("invalid PCI IOMMU");
                }
                for (index, window) in windows.iter().enumerate() {
                    let size = window
                        .cpu_range
                        .end
                        .checked_sub(window.cpu_range.start)
                        .filter(|&size| size != 0)
                        .ok_or("invalid PCI window")?;
                    let end = window
                        .bus_address
                        .checked_add(size)
                        .ok_or("PCI bus window overflow")?;
                    if !window.cpu_range.start.is_multiple_of(4096)
                        || !window.cpu_range.end.is_multiple_of(4096)
                        || !window.bus_address.is_multiple_of(4096)
                    {
                        return Err("PCI memory window is not page-aligned");
                    }
                    if window.space == PciAddressSpace::Memory32 && end > 1_u64 << 32 {
                        return Err("PCI window exceeds its address space");
                    }
                    if windows[..index].iter().any(|other| {
                        window.bus_address
                            < other.bus_address + (other.cpu_range.end - other.cpu_range.start)
                            && other.bus_address < end
                    }) {
                        return Err("overlapping PCI bus windows");
                    }
                }
            }
        }
        Ok(())
    }
}
