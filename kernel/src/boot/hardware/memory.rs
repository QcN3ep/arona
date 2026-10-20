// SPDX-License-Identifier: MPL-2.0

//! 物理内存区域及启动保留。

use arrayvec::ArrayVec;

pub const MAX_MEMORY_REGIONS: usize = 512;

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
