// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

pub const MAX_MEMORY_REGIONS: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryRegion {
    pub range: PhysRange,
    pub state: MemoryRegionState,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum MemoryRegionState {
    Available,
    /// Reserved during boot; reclaimable once confirmed unused.
    BootReserved,
    /// Always reserved.
    Reserved,
    /// Always reserved and must never be mapped.
    NoMap,
}

/// Interval `[start, end)` of physical byte addresses; a valid interval must satisfy `start < end`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysRange {
    pub start: u64,
    pub end: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VirtRange {
    pub start: usize,
    pub end: usize,
}

impl VirtRange {
    pub fn contains(self, other: Self) -> bool {
        self.start <= other.start && other.end <= self.end
    }

    pub fn overlaps(self, other: Self) -> bool {
        self.start < other.end && other.start < self.end
    }
}

impl PhysRange {
    /// Returns the 4 KiB page envelope covering the entire range.
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
