// SPDX-License-Identifier: MPL-2.0

//! 设备身份、拓扑、资源及配置属性。

use super::{PciWindow, PhysRange};
use arrayvec::{ArrayString, ArrayVec};
use core::ops::Range;

pub const MAX_DEVICES: usize = 128;
pub const MAX_MMIO_REGIONS: usize = 16;
pub const MAX_INTERRUPTS: usize = 16;
pub const MAX_PCI_WINDOWS: usize = 16;
pub const MAX_DEVICE_IDENTITIES: usize = 8;
pub const MAX_DEVICE_PROPERTIES: usize = 512;
pub const MAX_DEVICE_PROPERTY_BYTES: usize = 128;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HardwareDeviceInfo {
    pub instance_id: HardwareDeviceInstanceId,
    pub name: ArrayString<64>,
    pub parent: Option<HardwareDeviceInstanceId>,
    pub identities: ArrayVec<DeviceIdentity, MAX_DEVICE_IDENTITIES>,
    pub use_policy: DeviceUsePolicy,
    pub location: HardwareDeviceLocation,
    pub mmio_regions: ArrayVec<PhysRange, MAX_MMIO_REGIONS>,
    pub interrupts: ArrayVec<HardwareInterruptInfo, MAX_INTERRUPTS>,
    pub interrupt_route: Option<HardwareInterruptRoute>,
    pub interrupt_lines: Option<Range<u32>>,
    pub msi_vectors: Option<Range<u32>>,
    pub dma: Option<DmaInfo>,
}

impl HardwareDeviceInfo {
    pub const fn new(instance_id: HardwareDeviceInstanceId) -> Self {
        Self {
            instance_id,
            name: ArrayString::new_const(),
            parent: None,
            identities: ArrayVec::new_const(),
            use_policy: DeviceUsePolicy::KernelReserved,
            location: HardwareDeviceLocation::Platform,
            mmio_regions: ArrayVec::new_const(),
            interrupts: ArrayVec::new_const(),
            interrupt_route: None,
            interrupt_lines: None,
            msi_vectors: None,
            dma: None,
        }
    }

    pub fn has_identity(&self, namespace: &str, value: &str) -> bool {
        self.identities.iter().any(|identity| {
            identity.namespace.as_str() == namespace && identity.value.as_str() == value
        })
    }
}

/// 实例编号，1-base。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HardwareDeviceInstanceId(pub u32);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceIdentity {
    pub namespace: ArrayString<16>,
    pub value: ArrayString<64>,
}

impl DeviceIdentity {
    pub fn new(namespace: &str, value: &str) -> Result<Self, &'static str> {
        Ok(Self {
            namespace: namespace
                .try_into()
                .map_err(|_| "device identity namespace is too long")?,
            value: value
                .try_into()
                .map_err(|_| "device identity is too long")?,
        })
    }
}

/// 保留来源编码的自有属性，不授予资源访问权限。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceProperty {
    pub device: HardwareDeviceInstanceId,
    pub namespace: ArrayString<16>,
    pub name: ArrayString<64>,
    pub value: ArrayVec<u8, MAX_DEVICE_PROPERTY_BYTES>,
}

impl DeviceProperty {
    pub fn new(
        device: HardwareDeviceInstanceId,
        namespace: &str,
        name: &str,
        value: &[u8],
    ) -> Result<Self, &'static str> {
        Ok(Self {
            device,
            namespace: namespace
                .try_into()
                .map_err(|_| "device property namespace is too long")?,
            name: name
                .try_into()
                .map_err(|_| "device property name is too long")?,
            value: value
                .try_into()
                .map_err(|_| "device property is too large")?,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConsoleInfo {
    pub device: HardwareDeviceInstanceId,
    pub options: ArrayString<64>,
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
        windows: ArrayVec<PciWindow, MAX_PCI_WINDOWS>,
        iommu: Option<HardwareDeviceInstanceId>,
    },
}

/// 输入 n 对应父控制器的 base_line + n。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HardwareInterruptRoute {
    pub controller: HardwareDeviceInstanceId,
    pub base_line: u32,
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
