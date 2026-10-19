// SPDX-License-Identifier: MPL-2.0

//! 处理器、内存及设备的硬件描述。

use arrayvec::ArrayVec;

pub const MAX_CPUS: usize = 128;
pub const MAX_MEMORY_REGIONS: usize = 512;
pub const MAX_DEVICES: usize = 64;
pub const MAX_MMIO_REGIONS: usize = 16;
pub const MAX_INTERRUPTS: usize = 16;
pub const MAX_PCI_MEMORY_WINDOWS: usize = 16;

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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MemoryInfo {
    /// 按地址递增排列、互不重叠并覆盖全部 RAM 的区域表。
    pub regions: ArrayVec<MemoryRegion, MAX_MEMORY_REGIONS>,
}

impl MemoryInfo {
    /// 检查整个范围是否位于允许映射的 RAM 中。
    pub fn is_mappable(&self, range: PhysRange) -> bool {
        if range.start >= range.end {
            return false;
        }
        let mut cursor = range.start;
        for region in &self.regions {
            if region.range.end <= cursor {
                continue;
            }
            if region.range.start > cursor || region.state == MemoryRegionState::NoMap {
                return false;
            }
            cursor = region.range.end;
            if cursor >= range.end {
                return true;
            }
        }
        false
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryRegion {
    pub range: PhysRange,
    pub state: MemoryRegionState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryRegionState {
    Available,
    /// 启动期间保留，确认不再使用后可回收。
    BootReserved,
    /// 始终保留。
    Reserved,
    /// 始终保留，且不得建立映射。
    NoMap,
}

/// 物理字节地址的区间 `[start, end)`，有效区间应当满足 `start < end`。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysRange {
    pub start: u64,
    pub end: u64,
}

impl PhysRange {
    pub fn contains(self, other: Self) -> bool {
        self.start <= other.start && other.end <= self.end
    }

    pub fn overlaps(self, other: Self) -> bool {
        self.start < other.end && other.start < self.end
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HardwareDeviceInfo {
    pub instance_id: HardwareDeviceInstanceId,
    pub kind: HardwareDeviceKind,
    pub use_policy: DeviceUsePolicy,
    pub location: HardwareDeviceLocation,
    pub mmio_regions: ArrayVec<PhysRange, MAX_MMIO_REGIONS>,
    pub interrupts: ArrayVec<HardwareInterruptInfo, MAX_INTERRUPTS>,
    pub dma: Option<DmaInfo>,
}

impl HardwareDeviceInfo {
    fn addresses(&self) -> impl Iterator<Item = PhysRange> + '_ {
        let (ecam, windows): (_, &[PhysRange]) = match &self.location {
            HardwareDeviceLocation::PciHostBridge {
                ecam,
                memory_windows,
                ..
            } => (Some(*ecam), memory_windows.as_slice()),
            HardwareDeviceLocation::Platform => (None, &[]),
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
    PciHostBridge {
        segment: u16,
        first_bus: u8,
        last_bus: u8,
        ecam: PhysRange,
        memory_windows: ArrayVec<PhysRange, MAX_PCI_MEMORY_WINDOWS>,
        iommu: Option<HardwareDeviceInstanceId>,
    },
}

impl HardwareDeviceLocation {
    fn bus_range_overlaps(&self, other: &Self) -> bool {
        match (self, other) {
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
    pub isolation: Option<DmaIsolationRoute>,
}

/// IOMMU 实例及其请求源命名空间中的标识。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DmaIsolationRoute {
    pub iommu: HardwareDeviceInstanceId,
    pub requester_id: u32,
}
