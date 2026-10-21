// SPDX-License-Identifier: MPL-2.0

use core::ops::Range;
use spin::Mutex;

use super::{
    Frame, FrameAccess, FrameAllocation, FrameAllocationReference, MemoryError, ReferenceKind,
    VirtRange,
    frame_allocator::FrameAllocator,
    manager,
    mapping::{
        MappingIndex, MappingInfo, MappingJournal, MappingKind, MappingOptions, MappingRecord,
        MappingRights,
    },
    paging::{PageTable, PageTableState},
};
use crate::hardware::{
    self,
    mmu::{
        MemoryType, MmuError, PAGE_SIZE, PageAttributes, PageMapping, PagePermissions,
        PageTableEntry,
    },
};

pub const USER_ADDRESS_RANGE: VirtRange = VirtRange {
    start: 0x1_0000,
    end: 0x40_0000_0000,
};

/// # Safety
///
/// 操作及析构须满足 `Mmu` 的执行环境要求，析构前须让所有处理器切离该地址空间。
pub struct AddressSpace {
    table: PageTable,
    state: Mutex<AddressSpaceState>,
    bounds: VirtRange,
}

struct AddressSpaceState {
    mappings: MappingIndex,
    generation: u64,
}

impl AddressSpace {
    pub fn new() -> Result<Self, MemoryError> {
        let kernel = manager()
            .kernel_space
            .get()
            .ok_or(MemoryError::NotInitialized)?;
        Ok(Self::create(
            PageTable::user(&kernel.table)?,
            USER_ADDRESS_RANGE,
        ))
    }

    pub(super) fn kernel() -> Result<Self, MemoryError> {
        let table = PageTable::kernel()?;
        let layout = hardware::mmu().layout();
        let bits = PAGE_SIZE.trailing_zeros() + layout.index_bits * layout.levels as u32;
        let window_bits =
            PAGE_SIZE.trailing_zeros() + layout.index_bits * layout.window.level as u32;
        Ok(Self::create(
            table,
            VirtRange {
                start: usize::MAX << (bits - 1),
                end: layout.window.virtual_address & !((1 << window_bits) - 1),
            },
        ))
    }

    fn create(table: PageTable, bounds: VirtRange) -> Self {
        Self {
            table,
            state: Mutex::new(AddressSpaceState {
                mappings: MappingIndex::new(),
                generation: 0,
            }),
            bounds,
        }
    }

    pub fn root(&self) -> Frame {
        self.table.root()
    }

    pub fn activate(&self) -> Result<(), MemoryError> {
        hardware::mmu().switch_root(self.root())?;
        Ok(())
    }

    pub fn map(
        &self,
        allocation: &FrameAllocation,
        frames: Range<u64>,
        options: MappingOptions,
    ) -> Result<VirtRange, MemoryError> {
        self.map_reference(
            allocation.reference(ReferenceKind::Mapping)?,
            frames,
            options,
        )
    }

    pub fn map_reference(
        &self,
        reference: FrameAllocationReference,
        frames: Range<u64>,
        options: MappingOptions,
    ) -> Result<VirtRange, MemoryError> {
        let length = self.validate_source(&reference, &frames, options)?;
        let mut state = self.state.lock();
        let range = state
            .mappings
            .gap(self.bounds, length, options.alignment)
            .ok_or(MemoryError::NoMemory)?;
        self.map_locked(&mut state, range, &reference, frames, options)?;
        Ok(range)
    }

    pub fn map_at(
        &self,
        address: usize,
        allocation: &FrameAllocation,
        frames: Range<u64>,
        options: MappingOptions,
    ) -> Result<VirtRange, MemoryError> {
        self.map_reference_at(
            address,
            allocation.reference(ReferenceKind::Mapping)?,
            frames,
            options,
        )
    }

    pub fn map_reference_at(
        &self,
        address: usize,
        reference: FrameAllocationReference,
        frames: Range<u64>,
        options: MappingOptions,
    ) -> Result<VirtRange, MemoryError> {
        let length = self.validate_source(&reference, &frames, options)?;
        let range = VirtRange {
            start: address,
            end: address
                .checked_add(length)
                .ok_or(MemoryError::InvalidRequest)?,
        };
        self.validate_range(range)?;
        if !address.is_multiple_of(options.alignment) {
            return Err(MemoryError::InvalidRequest);
        }
        self.map_locked(&mut self.state.lock(), range, &reference, frames, options)?;
        Ok(range)
    }

    fn validate_source(
        &self,
        reference: &FrameAllocationReference,
        frames: &Range<u64>,
        options: MappingOptions,
    ) -> Result<usize, MemoryError> {
        let info = reference.info();
        if frames.start >= frames.end
            || frames.end > info.frames
            || info.purpose == super::FramePurpose::PageTable
            || (self.table.is_user() && info.owner == super::FrameOwner::Kernel)
            || !options.maximum.allows(options.permissions)
        {
            return Err(MemoryError::InvalidRequest);
        }
        validate_alignment(options.alignment)?;
        usize::try_from(frames.end - frames.start)
            .ok()
            .and_then(|count| count.checked_mul(PAGE_SIZE))
            .ok_or(MemoryError::InvalidRequest)
    }

    fn map_locked(
        &self,
        state: &mut AddressSpaceState,
        range: VirtRange,
        reference: &FrameAllocationReference,
        frames: Range<u64>,
        options: MappingOptions,
    ) -> Result<(), MemoryError> {
        if state.mappings.overlaps(range) {
            return Err(MemoryError::AlreadyMapped);
        }
        let user = self.table.is_user();
        let mut quota =
            manager().reserve_mapping_records(if user { pages(range).len() } else { 0 })?;
        let generation = state.next_generation()?;
        let result = self.table.edit(|table| {
            for (address, frame) in pages(range).zip(
                reference
                    .frames()
                    .skip(frames.start as usize)
                    .take((frames.end - frames.start) as usize),
            ) {
                let mut record = MappingRecord {
                    range: page_range(address),
                    maximum: options.maximum,
                    generation,
                    kind: MappingKind::Preparing,
                    journal: MappingJournal {
                        mapping: Some(PageMapping {
                            physical_address: frame.address(),
                            attributes: PageAttributes {
                                permissions: options.permissions,
                                memory_type: MemoryType::Normal,
                                user_accessible: user,
                            },
                        }),
                        ..MappingJournal::default()
                    },
                };
                state.mappings.insert(record)?;
                record.journal.tables = table.prepare(address)?.tables;
                state.mappings.set(record);
                let access = reference
                    .reference(ReferenceKind::Mapping)?
                    .access(frame, options.permissions.into())?;
                core::mem::forget(access);
                record.journal.referenced = true;
                state.mappings.set(record);
            }
            for address in pages(range) {
                let mut record = state.record(address);
                PageTableState::write_leaf(
                    address,
                    record.journal.tables[0].unwrap(),
                    record.journal.mapping,
                )?;
                record.journal.written = true;
                state.mappings.set(record);
            }
            Ok(())
        });
        if result.is_err() {
            self.table.edit(|table| state.rollback_map(table, range));
            return result;
        }
        hardware::mmu().invalidate_all();
        for address in pages(range) {
            let mut record = state.record(address);
            record.kind = MappingKind::Mapped;
            record.journal = MappingJournal::default();
            state.mappings.set(record);
        }
        quota.0 = 0;
        Ok(())
    }

    pub fn reserve(&self, length: usize, alignment: usize) -> Result<VirtRange, MemoryError> {
        validate_alignment(alignment)?;
        if length == 0 || !length.is_multiple_of(PAGE_SIZE) {
            return Err(MemoryError::InvalidRequest);
        }
        let mut state = self.state.lock();
        let range = state
            .mappings
            .gap(self.bounds, length, alignment)
            .ok_or(MemoryError::NoMemory)?;
        self.reserve_locked(&mut state, range)?;
        Ok(range)
    }

    pub fn reserve_at(&self, range: VirtRange) -> Result<(), MemoryError> {
        self.validate_range(range)?;
        self.reserve_locked(&mut self.state.lock(), range)
    }

    fn reserve_locked(
        &self,
        state: &mut AddressSpaceState,
        range: VirtRange,
    ) -> Result<(), MemoryError> {
        if state.mappings.overlaps(range) {
            return Err(MemoryError::AlreadyMapped);
        }
        let mut quota = manager().reserve_mapping_records(usize::from(self.table.is_user()))?;
        state.reserve(range)?;
        quota.0 = 0;
        Ok(())
    }

    pub fn unreserve(&self, range: VirtRange) -> Result<(), MemoryError> {
        self.validate_range(range)?;
        let mut state = self.state.lock();
        let record = state
            .mappings
            .get(range.start)
            .ok_or(MemoryError::NotMapped)?;
        if record.kind != MappingKind::Reserved || record.range != range {
            return Err(MemoryError::InUse);
        }
        state.mappings.remove(range.start);
        if self.table.is_user() {
            manager().release_mapping_records(1);
        }
        Ok(())
    }

    pub fn unmap(&self, range: VirtRange) -> Result<(), MemoryError> {
        self.validate_range(range)?;
        let mut state = self.state.lock();
        let staged = self.table.edit(|table| state.stage(table, range, None));
        if let Err(error) = staged {
            state.clear_journals(range);
            return Err(error);
        }
        let result = self.table.edit(|table| {
            for address in pages(range) {
                let mut record = state.record(address);
                PageTableState::write_leaf(address, record.journal.tables[0].unwrap(), None)?;
                record.journal.written = true;
                state.mappings.set(record);
            }
            for address in pages(range) {
                let mut record = state.record(address);
                let tables = record.journal.tables;
                table.detach_empty(address, &tables, |level| {
                    record.journal.detached |= 1 << level;
                    state.mappings.set(record);
                })?;
            }
            Ok(())
        });
        if let Err(error) = result {
            state.restore_entries(range);
            state.clear_journals(range);
            return Err(error);
        }
        hardware::mmu().invalidate_all();
        self.table.edit(|table| {
            for address in pages(range) {
                let record = state.record(address);
                table.release_tables(&record.journal.tables, record.journal.detached);
                manager()
                    .release_mapping(record.journal.frame(), record.journal.permissions().into())
                    .expect("lost mapping allocation");
            }
        });
        for address in pages(range) {
            state.mappings.remove(address);
        }
        if self.table.is_user() {
            manager().release_mapping_records(pages(range).len());
        }
        Ok(())
    }

    pub fn protect(
        &self,
        range: VirtRange,
        permissions: PagePermissions,
    ) -> Result<(), MemoryError> {
        self.validate_range(range)?;
        let mut state = self.state.lock();
        let generation = state.next_generation()?;
        let staged = self
            .table
            .edit(|table| state.stage(table, range, Some(permissions)));
        if let Err(error) = staged {
            state.clear_journals(range);
            return Err(error);
        }
        let mut allocator = manager().allocator.lock();
        // 同一页框的多个别名须一起撤销旧访问计数，才能一起切换权限。
        for address in pages(range) {
            let journal = state.record(address).journal;
            allocator.release_access(
                journal.frame(),
                ReferenceKind::Mapping,
                journal.permissions().into(),
            );
        }
        let result = (|| {
            for address in pages(range) {
                let mut record = state.record(address);
                let frame = record.journal.frame();
                let head = allocator.allocation_of(frame)?;
                allocator.reserve_access(
                    head,
                    frame,
                    ReferenceKind::Mapping,
                    permissions.into(),
                )?;
                record.journal.referenced = true;
                state.mappings.set(record);
            }
            for address in pages(range) {
                let mut record = state.record(address);
                PageTableState::write_leaf(address, record.journal.tables[0].unwrap(), None)?;
                record.journal.written = true;
                state.mappings.set(record);
            }
            hardware::mmu().invalidate_all();
            for address in pages(range) {
                let record = state.record(address);
                let mut mapping = record.journal.mapping.unwrap();
                if FrameAccess::from(permissions) == FrameAccess::Execute
                    && FrameAccess::from(mapping.attributes.permissions) != FrameAccess::Execute
                {
                    hardware::mmu().synchronize_frame(record.journal.frame())?;
                }
                mapping.attributes.permissions = permissions;
                PageTableState::write_leaf(
                    address,
                    record.journal.tables[0].unwrap(),
                    Some(mapping),
                )?;
            }
            Ok(())
        })();
        if result.is_err() {
            state.restore_entries(range);
            state.restore_access(&mut allocator, range, permissions);
        } else {
            hardware::mmu().invalidate_all();
            for address in pages(range) {
                let mut record = state.record(address);
                record.generation = generation;
                state.mappings.set(record);
            }
        }
        state.clear_journals(range);
        result
    }

    pub fn query(&self, address: usize) -> Result<Option<MappingInfo>, MemoryError> {
        if address < self.bounds.start || address >= self.bounds.end {
            return Err(MemoryError::InvalidRequest);
        }
        super::paging::validate_address(address, false)?;
        let state = self.state.lock();
        let Some(record) = state.mappings.get(address) else {
            return Ok(None);
        };
        let mapping = if record.kind == MappingKind::Reserved {
            None
        } else {
            self.table.query(address)?
        };
        Ok(Some(MappingInfo {
            range: record.range,
            mapping,
            maximum: record.maximum,
            generation: record.generation,
        }))
    }

    pub fn translate(&self, address: usize) -> Result<Option<u64>, MemoryError> {
        Ok(self.query(address)?.and_then(|info| {
            info.mapping
                .map(|mapping| mapping.physical_address + (address % PAGE_SIZE) as u64)
        }))
    }

    /// # Safety
    ///
    /// 后备存储须在内核运行期间持续有效。
    pub(crate) fn map_permanent(
        &self,
        address: usize,
        mapping: PageMapping,
    ) -> Result<(), MemoryError> {
        let range = VirtRange {
            start: address,
            end: address
                .checked_add(PAGE_SIZE)
                .ok_or(MemoryError::InvalidRequest)?,
        };
        self.validate_range(range)?;
        if self.table.is_user() {
            return Err(MemoryError::InvalidRequest);
        }
        let mut state = self.state.lock();
        let generation = state.next_generation()?;
        state.mappings.insert(MappingRecord {
            range,
            maximum: mapping.attributes.permissions.into(),
            generation,
            kind: MappingKind::Permanent,
            journal: MappingJournal::default(),
        })?;
        if let Err(error) = self.table.map_permanent(address, mapping) {
            state.mappings.remove(address);
            return Err(error);
        }
        Ok(())
    }

    fn validate_range(&self, range: VirtRange) -> Result<(), MemoryError> {
        if range.start >= range.end
            || !self.bounds.contains(range)
            || !range.start.is_multiple_of(PAGE_SIZE)
            || !range.end.is_multiple_of(PAGE_SIZE)
        {
            return Err(MemoryError::InvalidRequest);
        }
        super::paging::validate_address(range.start, true)?;
        super::paging::validate_address(range.end - 1, false)
    }
}

impl AddressSpaceState {
    fn next_generation(&mut self) -> Result<u64, MemoryError> {
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or(MemoryError::ReferenceOverflow)?;
        Ok(self.generation)
    }

    fn record(&self, address: usize) -> MappingRecord {
        self.mappings.get(address).expect("missing mapping record")
    }

    fn reserve(&mut self, range: VirtRange) -> Result<(), MemoryError> {
        let generation = self.next_generation()?;
        self.mappings.insert(MappingRecord {
            range,
            generation,
            maximum: MappingRights(0),
            kind: MappingKind::Reserved,
            journal: MappingJournal::default(),
        })
    }

    fn stage(
        &mut self,
        table: &PageTableState,
        range: VirtRange,
        permissions: Option<PagePermissions>,
    ) -> Result<(), MemoryError> {
        for address in pages(range) {
            let mut record = self.mappings.get(address).ok_or(MemoryError::NotMapped)?;
            if record.kind != MappingKind::Mapped {
                return Err(MemoryError::InUse);
            }
            if permissions.is_some_and(|permissions| !record.maximum.allows(permissions)) {
                return Err(MemoryError::AccessConflict);
            }
            let walk = table.walk(address)?;
            let PageTableEntry::Mapping(mapping) = walk.entry else {
                return Err(MmuError::InvalidTable.into());
            };
            record.journal = MappingJournal {
                tables: walk.tables,
                mapping: Some(mapping),
                ..MappingJournal::default()
            };
            self.mappings.set(record);
        }
        Ok(())
    }

    fn clear_journals(&mut self, range: VirtRange) {
        for address in pages(range) {
            let Some(mut record) = self.mappings.get(address) else {
                break;
            };
            if record.journal.mapping.is_none() {
                break;
            }
            record.journal = MappingJournal::default();
            self.mappings.set(record);
        }
    }

    fn restore_entries(&self, range: VirtRange) {
        for address in pages(range).rev() {
            if let Some(record) = self.mappings.get(address) {
                PageTableState::restore_tables(
                    address,
                    &record.journal.tables,
                    record.journal.detached,
                );
            }
        }
        for address in pages(range) {
            if let Some(record) = self.mappings.get(address)
                && record.journal.written
            {
                PageTableState::write_leaf(
                    address,
                    record.journal.tables[0].unwrap(),
                    record.journal.mapping,
                )
                .expect("failed to restore page mapping");
            }
        }
        hardware::mmu().invalidate_all();
    }

    fn rollback_map(&mut self, table: &mut PageTableState, range: VirtRange) {
        for address in pages(range) {
            let Some(record) = self.mappings.get(address) else {
                break;
            };
            if record.journal.written {
                PageTableState::write_leaf(address, record.journal.tables[0].unwrap(), None)
                    .expect("failed to roll back page mapping");
            }
        }
        hardware::mmu().invalidate_all();
        for address in pages(range) {
            let Some(record) = self.mappings.get(address) else {
                break;
            };
            if record.journal.referenced {
                manager()
                    .release_mapping(record.journal.frame(), record.journal.permissions().into())
                    .expect("lost mapping allocation");
            }
            table
                .prune_empty(address)
                .expect("failed to discard prepared page tables");
            self.mappings.remove(address);
        }
    }

    fn restore_access(
        &self,
        allocator: &mut FrameAllocator,
        range: VirtRange,
        permissions: PagePermissions,
    ) {
        for address in pages(range) {
            let record = self.record(address);
            if record.journal.referenced {
                allocator.release_access(
                    record.journal.frame(),
                    ReferenceKind::Mapping,
                    permissions.into(),
                );
            }
        }
        for address in pages(range) {
            let journal = self.record(address).journal;
            let frame = journal.frame();
            let head = allocator
                .allocation_of(frame)
                .expect("lost mapping allocation");
            allocator
                .reserve_access(
                    head,
                    frame,
                    ReferenceKind::Mapping,
                    journal.permissions().into(),
                )
                .expect("failed to restore frame access");
        }
    }
}

impl Drop for AddressSpace {
    fn drop(&mut self) {
        if self.table.is_user() {
            manager().release_mapping_records(self.state.lock().mappings.len());
        }
    }
}

fn pages(range: VirtRange) -> core::iter::StepBy<Range<usize>> {
    (range.start..range.end).step_by(PAGE_SIZE)
}

fn page_range(address: usize) -> VirtRange {
    VirtRange {
        start: address,
        end: address + PAGE_SIZE,
    }
}

fn validate_alignment(alignment: usize) -> Result<(), MemoryError> {
    if alignment < PAGE_SIZE || !alignment.is_power_of_two() {
        return Err(MemoryError::InvalidRequest);
    }
    Ok(())
}
