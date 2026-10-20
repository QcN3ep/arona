// SPDX-License-Identifier: MPL-2.0

//! 设备资源、寄存器配置及 DMA 描述。

use super::{PciWindow, PhysRange};
use arrayvec::ArrayVec;
use core::ops::Range;

pub const MAX_DEVICES: usize = 128;
pub const MAX_MMIO_REGIONS: usize = 16;
pub const MAX_INTERRUPTS: usize = 16;
pub const MAX_PCI_WINDOWS: usize = 16;

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
    pub const SYSCON: Self = Self(11);
    pub const RISCV_CLINT: Self = Self(12);
    pub const GOLDFISH_RTC: Self = Self(13);
    pub const QEMU_FW_CFG: Self = Self(14);
    pub const CFI_FLASH: Self = Self(15);
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeviceConfiguration {
    None,
    Uart(UartInfo),
    InterruptController {
        /// 控制器命名空间内可编码的中断号。
        lines: Range<u32>,
    },
    Msi {
        vectors: Range<u32>,
    },
    Syscon(SysconInfo),
    Flash {
        bank_width: u8,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UartInfo {
    pub register_shift: u8,
    pub register_width: u8,
    pub register_offset: u32,
    pub endian: RegisterEndian,
    pub clock_frequency_hz: Option<u64>,
    pub current_speed: Option<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegisterEndian {
    Little,
    Big,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SysconInfo {
    pub register_width: u8,
    pub endian: RegisterEndian,
    pub poweroff: Option<RegisterWrite>,
    pub reboot: Option<RegisterWrite>,
}

/// 以 `(old & !mask) | (value & mask)` 更新寄存器。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegisterWrite {
    pub offset: u32,
    pub mask: u32,
    pub value: u32,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ConsoleInfo {
    #[default]
    Unspecified,
    Uart {
        device: HardwareDeviceInstanceId,
        settings: Option<SerialSettings>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SerialSettings {
    pub baud_rate: u32,
    pub parity: SerialParity,
    pub data_bits: u8,
    pub flow_control: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SerialParity {
    None,
    Even,
    Odd,
}
