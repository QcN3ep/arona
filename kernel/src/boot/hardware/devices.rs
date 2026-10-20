// SPDX-License-Identifier: MPL-2.0

//! 设备资源、寄存器配置及 DMA 描述。

use super::PhysRange;
use arrayvec::ArrayVec;
use core::ops::Range;

pub const MAX_DEVICES: usize = 128;
pub const MAX_MMIO_REGIONS: usize = 16;
pub const MAX_INTERRUPTS: usize = 16;
pub const MAX_PCI_MEMORY_WINDOWS: usize = 16;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HardwareDeviceInfo {
    pub instance_id: HardwareDeviceInstanceId,
    pub kind: HardwareDeviceKind,
    pub use_policy: DeviceUsePolicy,
    pub location: HardwareDeviceLocation,
    pub mmio_regions: ArrayVec<PhysRange, MAX_MMIO_REGIONS>,
    pub interrupts: ArrayVec<HardwareInterruptInfo, MAX_INTERRUPTS>,
    pub interrupt_route: Option<HardwareInterruptRoute>,
    pub configuration: DeviceConfiguration,
    pub dma: Option<DmaInfo>,
}

impl HardwareDeviceInfo {
    pub(super) fn validate_configuration(&self) -> Result<(), &'static str> {
        self.validate_location()?;
        if self.use_policy == DeviceUsePolicy::UserAssignable
            && matches!(
                self.kind,
                HardwareDeviceKind::RISCV_PLIC
                    | HardwareDeviceKind::RISCV_CLINT
                    | HardwareDeviceKind::LOONGARCH_EIOINTC
                    | HardwareDeviceKind::LOONGARCH_PCH_PIC
                    | HardwareDeviceKind::LOONGARCH_PCH_MSI
                    | HardwareDeviceKind::PCI_HOST_BRIDGE
                    | HardwareDeviceKind::IOMMU
            )
        {
            return Err("system device must be reserved for the kernel");
        }
        let first_size = self
            .mmio_regions
            .first()
            .map(|range| range.end.saturating_sub(range.start))
            .unwrap_or(0);
        match (&self.configuration, self.kind) {
            (DeviceConfiguration::InterruptController { lines }, kind) => {
                let valid = match kind {
                    HardwareDeviceKind::RISCV_PLIC => {
                        lines.start == 1 && (2..=1024).contains(&lines.end)
                    }
                    HardwareDeviceKind::LOONGARCH_EIOINTC => *lines == (0..256),
                    HardwareDeviceKind::LOONGARCH_PCH_PIC => {
                        *lines == (0..64) && self.interrupt_route.is_some()
                    }
                    _ => false,
                };
                if !valid {
                    return Err("invalid interrupt controller namespace");
                }
            }
            (DeviceConfiguration::Msi { vectors }, HardwareDeviceKind::LOONGARCH_PCH_MSI) => {
                if vectors.is_empty()
                    || vectors.end > 256
                    || self.interrupt_route.is_none()
                    || first_size < 4
                {
                    return Err("invalid MSI controller configuration");
                }
            }
            (DeviceConfiguration::None, kind)
                if !matches!(
                    kind,
                    HardwareDeviceKind::RISCV_PLIC
                        | HardwareDeviceKind::LOONGARCH_EIOINTC
                        | HardwareDeviceKind::LOONGARCH_PCH_PIC
                        | HardwareDeviceKind::LOONGARCH_PCH_MSI
                ) => {}
            _ => return Err("device configuration does not match its kind"),
        }
        Ok(())
    }

    fn validate_location(&self) -> Result<(), &'static str> {
        let location_valid = match (&self.location, self.kind) {
            (HardwareDeviceLocation::Iocsr { .. }, HardwareDeviceKind::LOONGARCH_EIOINTC)
            | (HardwareDeviceLocation::PciHostBridge { .. }, HardwareDeviceKind::PCI_HOST_BRIDGE) => {
                true
            }
            (HardwareDeviceLocation::Platform, kind) => !matches!(
                kind,
                HardwareDeviceKind::LOONGARCH_EIOINTC | HardwareDeviceKind::PCI_HOST_BRIDGE
            ),
            _ => false,
        };
        if !location_valid {
            return Err("device location does not match its kind");
        }
        Ok(())
    }

    pub(super) fn addresses(&self) -> impl Iterator<Item = PhysRange> + '_ {
        let (ecam, windows): (_, &[PhysRange]) = match &self.location {
            HardwareDeviceLocation::PciHostBridge {
                ecam,
                memory_windows,
                ..
            } => (Some(*ecam), memory_windows.as_slice()),
            HardwareDeviceLocation::Platform | HardwareDeviceLocation::Iocsr { .. } => (None, &[]),
        };
        self.mmio_regions
            .iter()
            .copied()
            .chain(ecam)
            .chain(windows.iter().copied())
    }
}

/// 实例编号，1-base。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HardwareDeviceInstanceId(pub u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HardwareDeviceKind(pub u32);

impl HardwareDeviceKind {
    pub const UNKNOWN: Self = Self(0);
    pub const UART_16550: Self = Self(1);
    pub const PCI_HOST_BRIDGE: Self = Self(2);
    pub const RISCV_PLIC: Self = Self(3);
    pub const LOONGARCH_EIOINTC: Self = Self(4);
    pub const LOONGARCH_LS7A_RTC: Self = Self(5);
    pub const IOMMU: Self = Self(6);
    pub const VIRTIO_MMIO: Self = Self(7);
    pub const VIRTIO_PCI: Self = Self(8);
    pub const LOONGARCH_PCH_PIC: Self = Self(9);
    pub const LOONGARCH_PCH_MSI: Self = Self(10);
    pub const RISCV_CLINT: Self = Self(12);
}

/// 设备资源是否允许委托给用户态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceUsePolicy {
    KernelReserved,
    UserAssignable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(
    clippy::large_enum_variant,
    reason = "启动期禁止动态分配，PCI 窗口必须内联保存"
)]
pub enum HardwareDeviceLocation {
    Platform,
    Iocsr {
        registers: Range<u32>,
    },
    PciHostBridge {
        segment: u16,
        first_bus: u8,
        last_bus: u8,
        ecam: PhysRange,
        memory_windows: ArrayVec<PhysRange, MAX_PCI_MEMORY_WINDOWS>,
        iommu: Option<HardwareDeviceInstanceId>,
    },
}

/// 输入 n 对应父控制器的 base_line + n。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HardwareInterruptRoute {
    pub controller: HardwareDeviceInstanceId,
    pub base_line: u32,
}

impl HardwareDeviceLocation {
    pub(super) fn overlaps(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Iocsr { registers: a }, Self::Iocsr { registers: b }) => {
                a.start < b.end && b.start < a.end
            }
            (
                Self::PciHostBridge {
                    segment: a_segment,
                    first_bus: a_first,
                    last_bus: a_last,
                    ..
                },
                Self::PciHostBridge {
                    segment: b_segment,
                    first_bus: b_first,
                    last_bus: b_last,
                    ..
                },
            ) => a_segment == b_segment && a_last >= b_first && b_last >= a_first,
            _ => false,
        }
    }
}

/// 中断控制器命名空间内的输入。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HardwareInterruptInfo {
    pub controller: HardwareDeviceInstanceId,
    pub line: u32,
    pub trigger: Option<InterruptTrigger>,
    pub polarity: Option<InterruptPolarity>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterruptTrigger {
    Edge,
    Level,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterruptPolarity {
    ActiveHigh,
    ActiveLow,
}

/// 已确认的 DMA 寻址能力；不存在时不得授权 DMA。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DmaInfo {
    /// 有效范围为 1 至 64。
    pub address_bits: u8,
    pub coherent: bool,
    pub isolation: Option<DmaIsolationRoute>,
}

/// IOMMU 实例及其请求源命名空间中的标识。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DmaIsolationRoute {
    pub iommu: HardwareDeviceInstanceId,
    pub requester_id: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeviceConfiguration {
    None,
    InterruptController {
        /// 控制器命名空间内可编码的中断号。
        lines: Range<u32>,
    },
    Msi {
        vectors: Range<u32>,
    },
}
