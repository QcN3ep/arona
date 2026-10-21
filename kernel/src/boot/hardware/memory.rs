// SPDX-License-Identifier: MPL-2.0

//! 启动内存区域表及保留操作。

use arrayvec::ArrayVec;

use crate::memory::{MAX_MEMORY_REGIONS, MemoryRegion, MemoryRegionState, PhysRange};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MemoryInfo {
    /// 按地址递增排列、互不重叠并覆盖全部 RAM 的区域表。
    pub regions: ArrayVec<MemoryRegion, MAX_MEMORY_REGIONS>,
}

impl MemoryInfo {
    pub fn reserve(
        &mut self,
        range: PhysRange,
        state: MemoryRegionState,
    ) -> Result<(), &'static str> {
        let range = range.page_envelope()?;
        if state == MemoryRegionState::Available || !self.is_mappable(range) {
            return Err("reservation is not contained in mappable RAM");
        }
        let extra = self
            .regions
            .iter()
            .filter(|region| region.state < state)
            .map(|region| {
                usize::from(region.range.start < range.start && range.start < region.range.end)
                    + usize::from(region.range.start < range.end && range.end < region.range.end)
            })
            .sum::<usize>();
        if self.regions.len() + extra > self.regions.capacity() {
            return Err("too many memory regions after reservation");
        }
        for boundary in [range.start, range.end] {
            if let Some(index) = self.regions.iter().position(|region| {
                region.state < state && region.range.start < boundary && boundary < region.range.end
            }) {
                let mut right = self.regions[index];
                right.range.start = boundary;
                self.regions[index].range.end = boundary;
                self.regions.insert(index + 1, right);
            }
        }
        for region in &mut self.regions {
            if range.contains(region.range) && region.state < state {
                region.state = state;
            }
        }
        let mut index = 1;
        while index < self.regions.len() {
            if self.regions[index - 1].range.end == self.regions[index].range.start
                && self.regions[index - 1].state == self.regions[index].state
            {
                self.regions[index - 1].range.end = self.regions.remove(index).range.end;
            } else {
                index += 1;
            }
        }
        Ok(())
    }

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
