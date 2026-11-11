// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::{DeviceError, PciHost};
use crate::{
    HardwareDeviceLocation, HardwareInfo,
    memory::{MemoryError, PhysRange},
};
use alloc::vec::Vec;

#[derive(Clone, Copy)]
pub(super) struct Assignment {
    pub(super) range: PhysRange,
    pub(super) host_window: usize,
}

#[derive(Clone, Copy)]
struct FreeRange {
    assignment: Assignment,
    prefetchable: bool,
}

pub(super) struct Pool(Vec<FreeRange>);

impl Pool {
    pub(super) fn root(host: &PciHost<'_>, hardware: &HardwareInfo) -> Result<Self, DeviceError> {
        let mut pool = Self(Vec::new());
        pool.0
            .try_reserve_exact(host.windows.len())
            .map_err(|_| MemoryError::NoMemory)?;
        for (index, window) in host.windows.iter().enumerate() {
            let end = window
                .bus_address
                .checked_add(window.cpu_range.end - window.cpu_range.start)
                .ok_or(DeviceError::InvalidArgument)?;
            pool.0.push(FreeRange {
                assignment: Assignment {
                    range: PhysRange {
                        start: window.bus_address,
                        end,
                    },
                    host_window: index,
                },
                prefetchable: window.prefetchable,
            });
        }
        for region in &hardware.memory.regions {
            pool.exclude(host, region.range)?;
        }
        for device in &hardware.devices {
            for range in &device.mmio_regions {
                pool.exclude(host, *range)?;
            }
            if let HardwareDeviceLocation::PciHostBridge { ecam, .. } = device.location {
                pool.exclude(host, ecam)?;
            }
        }
        Ok(pool)
    }

    pub(super) fn bridge(windows: [Option<Assignment>; 2]) -> Result<Self, DeviceError> {
        let mut pool = Self(Vec::new());
        pool.0
            .try_reserve_exact(2)
            .map_err(|_| MemoryError::NoMemory)?;
        for (kind, window) in windows.into_iter().enumerate() {
            if let Some(assignment) = window {
                pool.0.push(FreeRange {
                    assignment,
                    prefetchable: kind == 1,
                });
            }
        }
        Ok(pool)
    }

    fn exclude(&mut self, host: &PciHost<'_>, physical: PhysRange) -> Result<(), DeviceError> {
        let mut index = 0;
        while index < self.0.len() {
            let free = self.0[index];
            let window = host.windows[free.assignment.host_window];
            let start = physical.start.max(window.cpu_range.start);
            let end = physical.end.min(window.cpu_range.end);
            if start < end {
                let blocked = PhysRange {
                    start: window.bus_address + (start - window.cpu_range.start),
                    end: window.bus_address + (end - window.cpu_range.start),
                };
                if let Some(overlap) = intersection(free.assignment.range, blocked) {
                    self.remove(index, overlap)?;
                    continue;
                }
            }
            index += 1;
        }
        Ok(())
    }

    fn remove(&mut self, index: usize, range: PhysRange) -> Result<Assignment, DeviceError> {
        self.0.try_reserve(1).map_err(|_| MemoryError::NoMemory)?;
        let free = self.0.swap_remove(index);
        if free.assignment.range.start < range.start {
            self.0.push(FreeRange {
                assignment: Assignment {
                    range: PhysRange {
                        start: free.assignment.range.start,
                        end: range.start,
                    },
                    ..free.assignment
                },
                ..free
            });
        }
        if range.end < free.assignment.range.end {
            self.0.push(FreeRange {
                assignment: Assignment {
                    range: PhysRange {
                        start: range.end,
                        end: free.assignment.range.end,
                    },
                    ..free.assignment
                },
                ..free
            });
        }
        Ok(Assignment {
            range,
            ..free.assignment
        })
    }

    pub(super) fn claim(
        &mut self,
        range: PhysRange,
        prefetchable: bool,
    ) -> Result<Option<Assignment>, DeviceError> {
        let index = self.0.iter().position(|free| {
            free.assignment.range.contains(range) && (prefetchable || !free.prefetchable)
        });
        index.map(|index| self.remove(index, range)).transpose()
    }

    pub(super) fn allocate(
        &mut self,
        size: u64,
        alignment: u64,
        limit: u64,
        prefetchable: bool,
    ) -> Result<Option<Assignment>, DeviceError> {
        for fallback in [false, true] {
            for index in 0..self.0.len() {
                let free = self.0[index];
                if free.prefetchable != (prefetchable && !fallback) {
                    continue;
                }
                let Some(start) = free
                    .assignment
                    .range
                    .start
                    .checked_add(alignment - 1)
                    .map(|start| start & !(alignment - 1))
                else {
                    continue;
                };
                let Some(end) = start.checked_add(size) else {
                    continue;
                };
                if end <= free.assignment.range.end && end <= limit {
                    return self.remove(index, PhysRange { start, end }).map(Some);
                }
            }
        }
        Ok(None)
    }
}

fn intersection(left: PhysRange, right: PhysRange) -> Option<PhysRange> {
    let start = left.start.max(right.start);
    let end = left.end.min(right.end);
    (start < end).then_some(PhysRange { start, end })
}
