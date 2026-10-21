// SPDX-License-Identifier: MPL-2.0

use arrayvec::ArrayVec;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::{Mutex, Once};

use super::{
    AddressSpace, Frame, MAX_MEMORY_REGIONS, MemoryError, MemoryRegion, MemoryRegionState,
    PhysRange, VirtRange, manager,
    paging::{PageTable, PageTableState},
};
use crate::hardware::{
    self,
    mmu::{
        MemoryType, MmuError, PAGE_SIZE, PageAttributes, PageMapping, PagePermissions,
        PageTableEntry,
    },
};

pub(super) struct DirectMap {
    regions: Mutex<ArrayVec<MemoryRegion, MAX_MEMORY_REGIONS>>,
    layout: Once<DirectMapLayout>,
    active: AtomicBool,
}

pub(super) struct DirectMapLayout {
    pub range: VirtRange,
    physical_start: u64,
    root: Frame,
}

impl DirectMap {
    pub const fn new() -> Self {
        Self {
            regions: Mutex::new(ArrayVec::new_const()),
            layout: Once::new(),
            active: AtomicBool::new(false),
        }
    }

    pub fn set_regions(&self, regions: &[MemoryRegion]) {
        let mut stored = self.regions.lock();
        stored.clear();
        stored
            .try_extend_from_slice(regions)
            .expect("validated RAM directory exceeds capacity");
    }

    pub fn initialize(&self, space: &AddressSpace) -> Result<(), MemoryError> {
        let mut initialized = false;
        self.layout.try_call_once(|| {
            let layout = space.install_direct_map(self)?;
            initialized = true;
            Ok::<_, MemoryError>(layout)
        })?;
        if initialized {
            Ok(())
        } else {
            Err(MemoryError::AlreadyInitialized)
        }
    }

    pub fn range(&self) -> Option<VirtRange> {
        self.layout.get().map(|layout| layout.range)
    }

    pub fn activate(&self) {
        if self.layout.get().is_some() {
            self.active.store(true, Ordering::Release);
        }
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    pub fn active_address(&self, frame: Frame) -> Option<usize> {
        if !self.is_active() {
            return None;
        }
        self.address(frame)
    }

    fn address(&self, frame: Frame) -> Option<usize> {
        let layout = self.layout.get()?;
        let regions = self.regions.lock();
        let index = regions.partition_point(|region| region.range.end <= frame.address());
        let region = regions.get(index)?;
        if frame.address() < region.range.start || region.state == MemoryRegionState::NoMap {
            return None;
        }
        Some(layout.range.start + (frame.address() - layout.physical_start) as usize)
    }

    pub fn physical_range(&self) -> Result<PhysRange, MemoryError> {
        let regions = self.regions.lock();
        let range = PhysRange {
            start: regions.first().ok_or(MemoryError::NoMemory)?.range.start,
            end: regions.last().ok_or(MemoryError::NoMemory)?.range.end,
        };
        let bits = hardware::mmu().physical_address_bits();
        if bits < 64 && range.end > 1u64 << bits {
            return Err(MmuError::InvalidAddress.into());
        }
        Ok(range)
    }

    fn frames(regions: &[MemoryRegion]) -> impl Iterator<Item = (u64, MemoryRegionState)> + '_ {
        regions
            .iter()
            .filter(|region| region.state != MemoryRegionState::NoMap)
            .flat_map(|region| {
                (region.range.start..region.range.end)
                    .step_by(PAGE_SIZE)
                    .map(move |address| (address, region.state))
            })
    }

    pub fn populate(
        &self,
        table: &PageTable,
        range: VirtRange,
    ) -> Result<DirectMapLayout, MemoryError> {
        let physical_start = self.physical_range()?.start;
        let layout = DirectMapLayout {
            range,
            physical_start,
            root: table.root(),
        };
        let regions = self.regions.lock();
        table.edit(|table| {
            let mut installed_end = physical_start;
            let mut prepared = None;
            let mut leaf = None;
            let result: Result<(), MemoryError> = (|| {
                for (physical, state) in Self::frames(&regions) {
                    let address = range.start + (physical - physical_start) as usize;
                    prepared = Some(address);
                    let block = address
                        >> (PAGE_SIZE.trailing_zeros() + hardware::mmu().layout().index_bits);
                    if leaf.is_none_or(|(previous, _)| previous != block) {
                        leaf = Some((block, table.prepare(address)?.tables[0].unwrap()));
                    }
                    let frame = Frame::new(physical).unwrap();
                    let writable = {
                        let allocator = manager().allocator().lock();
                        if state == MemoryRegionState::Available {
                            !allocator.has_executors(frame)
                        } else {
                            allocator.has_writers(frame)
                        }
                    };
                    PageTableState::write_leaf(
                        address,
                        leaf.unwrap().1,
                        Some(PageMapping {
                            physical_address: physical,
                            attributes: PageAttributes {
                                permissions: if writable {
                                    PagePermissions::ReadWrite
                                } else {
                                    PagePermissions::ReadOnly
                                },
                                memory_type: MemoryType::Normal,
                                user_accessible: false,
                            },
                        }),
                    )?;
                    installed_end = physical + PAGE_SIZE as u64;
                }
                Ok(())
            })();
            if result.is_err() {
                Self::rollback(&regions, table, &layout, installed_end, prepared);
            }
            result
        })?;
        hardware::mmu().invalidate_all();
        Ok(layout)
    }

    fn rollback(
        regions: &[MemoryRegion],
        table: &mut PageTableState,
        layout: &DirectMapLayout,
        end: u64,
        prepared: Option<usize>,
    ) {
        for (physical, _) in Self::frames(regions).take_while(|(physical, _)| *physical < end) {
            let address = layout.range.start + (physical - layout.physical_start) as usize;
            let walk = table
                .walk(address)
                .expect("failed to inspect direct mapping");
            PageTableState::write_leaf(address, walk.tables[0].unwrap(), None)
                .expect("failed to roll back direct mapping");
        }
        hardware::mmu().invalidate_all();
        let mut previous = None;
        for (physical, _) in Self::frames(regions).take_while(|(physical, _)| *physical < end) {
            let address = layout.range.start + (physical - layout.physical_start) as usize;
            let block =
                address >> (PAGE_SIZE.trailing_zeros() + hardware::mmu().layout().index_bits);
            if previous != Some(block) {
                table
                    .prune_empty(address)
                    .expect("failed to discard direct map tables");
                previous = Some(block);
            }
        }
        if let Some(address) = prepared {
            table
                .prune_empty(address)
                .expect("failed to discard prepared direct map table");
        }
    }

    pub fn set_writable(&self, frame: Frame, writable: bool) -> Result<bool, MemoryError> {
        let Some(layout) = self.layout.get() else {
            return Ok(false);
        };
        let address = self.address(frame).ok_or(MemoryError::InvalidFrame)?;
        // aperture 的分支和叶条目永久保留，权限修改由页框分配器锁串行化。
        let walk = PageTableState::walk_from(layout.root, address)?;
        let PageTableEntry::Mapping(mut mapping) = walk.entry else {
            return Err(MmuError::InvalidTable.into());
        };
        if mapping.physical_address != frame.address() {
            return Err(MmuError::InvalidTable.into());
        }
        let previous = mapping.attributes.permissions == PagePermissions::ReadWrite;
        if previous != writable {
            let original = mapping;
            mapping.attributes.permissions = if writable {
                PagePermissions::ReadWrite
            } else {
                PagePermissions::ReadOnly
            };
            PageTableState::write_leaf(address, walk.tables[0].unwrap(), Some(mapping))?;
            if let Err(error) = hardware::mmu().invalidate_page(address) {
                PageTableState::write_leaf(address, walk.tables[0].unwrap(), Some(original))
                    .expect("failed to restore direct map entry");
                hardware::mmu().invalidate_all();
                return Err(error.into());
            }
        }
        Ok(previous)
    }
}
