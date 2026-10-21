// SPDX-License-Identifier: MPL-2.0

pub const MAX_MEMORY_REGIONS: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryRegion {
    pub range: PhysRange,
    pub state: MemoryRegionState,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
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
    /// 返回覆盖整个范围的 4 KiB 页包络。
    pub fn page_envelope(self) -> Result<Self, &'static str> {
        if self.start >= self.end {
            return Err("physical range is empty");
        }
        Ok(Self {
            start: self.start & !4095,
            end: self.end.checked_add(4095).ok_or("page envelope overflow")? & !4095,
        })
    }

    pub fn contains(self, other: Self) -> bool {
        self.start <= other.start && other.end <= self.end
    }

    pub fn overlaps(self, other: Self) -> bool {
        self.start < other.end && other.start < self.end
    }
}
