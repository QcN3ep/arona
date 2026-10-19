// SPDX-License-Identifier: MPL-2.0

//! 启动数据定义。

/// 物理字节地址的左闭右开区间 `[start, end)`。
///
/// 有效区间满足 `start < end`。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysRange {
    /// 起始地址，包含。
    pub start: u64,
    /// 结束地址，不包含。
    pub end: u64,
}

/// 启动时 RAM 区域的分配状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryRegionState {
    /// 校验后可分配。
    Available,
    /// 启动期间保留，确认不再使用后可回收。
    BootReserved,
    /// 始终保留。
    Reserved,
}

/// RAM 区域及其分配状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryRegion {
    pub range: PhysRange,
    pub state: MemoryRegionState,
}

/// 进入内核的处理器信息。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuInfo {
    /// 固件提供的硬件 ID；不保证连续，不能直接用作 CPU 数组下标。
    pub hardware_id: u64,
}

/// 启动时物理 RAM 描述。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryInfo<'boot> {
    /// 按地址递增排列、互不重叠并覆盖全部 RAM 的区域表。
    pub regions: &'boot [MemoryRegion],
}

/// 架构入口交给内核的规范化启动数据。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootInfo<'boot> {
    pub cpu: CpuInfo,
    pub memory: MemoryInfo<'boot>,
}

pub fn boot_kernel(_info: BootInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
