// SPDX-License-Identifier: MPL-2.0

use core::ops::Range;
use spin::Mutex;

use super::{
    DeviceMapping, Frame, FrameAccess, FrameAllocation, FrameAllocationReference, MemoryError,
    PhysRange, ReferenceKind, VirtRange,
    direct_map::{DirectMap, DirectMapLayout},
    frame_allocator::FrameAllocator,
    manager,
    mapping::{
        MappingIndex, MappingInfo, MappingJournal, MappingKind, MappingOptions, MappingRecord,
        MappingRights, OwnedMapping,
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
        manager().direct_map.activate();
        Ok(())
    }

    pub(super) fn install_direct_map(
        &self,
        direct_map: &DirectMap,
    ) -> Result<DirectMapLayout, MemoryError> {
        let physical = direct_map.physical_range()?;
        let length =
            usize::try_from(physical.end - physical.start).map_err(|_| MemoryError::NoMemory)?;
        let mut state = self.state.lock();
        let range = state
            .mappings
            .gap(self.bounds, length, PAGE_SIZE)
            .ok_or(MemoryError::NoMemory)?;
        let generation = state.next_generation()?;
        state.mappings.insert(MappingRecord {
            range,
            maximum: MappingRights::READ | MappingRights::WRITE,
            generation,
            kind: MappingKind::DirectMap,
            journal: MappingJournal::default(),
        })?;
        match direct_map.populate(&self.table, range) {
            Ok(layout) => Ok(layout),
            Err(error) => {
                state.mappings.remove(range.start);
                Err(error)
            }
        }
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
        self.map_locked(
            &mut state,
            range,
            &reference,
            frames,
            options,
            MappingKind::Mapped,
        )?;
        Ok(range)
    }

    pub(super) fn map_owned(
        &self,
        allocation: &FrameAllocation,
        frames: Range<u64>,
        options: MappingOptions,
    ) -> Result<OwnedMapping, MemoryError> {
        if self.table.is_user() {
            return Err(MemoryError::InvalidRequest);
        }
        let reference = allocation.reference(ReferenceKind::Mapping)?;
        let length = self.validate_source(&reference, &frames, options)?;
        let mut state = self.state.lock();
        let range = state
            .mappings
            .gap(self.bounds, length, options.alignment)
            .ok_or(MemoryError::NoMemory)?;
        self.map_locked(
            &mut state,
            range,
            &reference,
            frames,
            options,
            MappingKind::Owned,
        )?;
        Ok(OwnedMapping {
            range,
            generation: state.record(range.start).generation,
        })
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
        self.map_locked(
            &mut self.state.lock(),
            range,
            &reference,
            frames,
            options,
            MappingKind::Mapped,
        )?;
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
        kind: MappingKind,
    ) -> Result<(), MemoryError> {
        if state.mappings.overlaps(range) {
            return Err(MemoryError::AlreadyMapped);
        }
        let user = self.table.is_user();
        let mut quota =
            manager().reserve_mapping_records(if user { pages(range).len() } else { 0 })?;
        let generation = state.next_generation()?;
        let result = (|| {
            self.table.edit(|table| {
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
                }
                Ok::<_, MemoryError>(())
            })?;
            for address in pages(range) {
                let mut record = state.record(address);
                let access = reference
                    .reference(ReferenceKind::Mapping)?
                    .access(record.journal.frame(), options.permissions.into())?;
                record.journal.direct_was_writable = access.direct_was_writable;
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
        })();
        if result.is_err() {
            self.table.edit(|table| state.rollback_map(table, range));
            return result;
        }
        hardware::mmu().invalidate_all();
        for address in pages(range) {
            let mut record = state.record(address);
            record.kind = kind;
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
        self.unmap_inner(range, None)
    }

    pub(super) fn unmap_owned(&self, mapping: &OwnedMapping) -> Result<(), MemoryError> {
        self.unmap_inner(mapping.range, Some(mapping.generation))
    }

    fn unmap_inner(&self, range: VirtRange, owner: Option<u64>) -> Result<(), MemoryError> {
        self.validate_range(range)?;
        let mut state = self.state.lock();
        let staged = self
            .table
            .edit(|table| state.stage(table, range, None, owner));
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
            .edit(|table| state.stage(table, range, Some(permissions), None));
        if let Err(error) = staged {
            state.clear_journals(range);
            return Err(error);
        }
        if let Err(error) = state.withdraw_entries(range) {
            state.restore_entries(range);
            state.clear_journals(range);
            return Err(error);
        }
        let mut allocator = manager().allocator().lock();
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
                record.journal.direct_was_writable = manager().reserve_access(
                    &mut allocator,
                    head,
                    frame,
                    ReferenceKind::Mapping,
                    permissions.into(),
                )?;
                record.journal.referenced = true;
                state.mappings.set(record);
            }
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
            state
                .withdraw_entries(range)
                .expect("failed to withdraw changed mappings");
            state.restore_access(&mut allocator, range, permissions);
            state.restore_entries(range);
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

    pub(super) fn map_device(&self, physical: PhysRange) -> Result<(VirtRange, u64), MemoryError> {
        let mmu = hardware::mmu();
        let bits = mmu.physical_address_bits();
        if self.table.is_user() || bits < 64 && physical.end > 1u64 << bits {
            return Err(MemoryError::InvalidRequest);
        }
        let length = usize::try_from(physical.end - physical.start)
            .map_err(|_| MemoryError::InvalidRequest)?;
        let attributes = PageAttributes {
            permissions: PagePermissions::ReadWrite,
            memory_type: mmu.device_memory_type(),
            user_accessible: false,
        };
        let mut state = self.state.lock();
        if state.mappings.any(|record| {
            let (MappingKind::Device { physical_start }
            | MappingKind::Permanent { physical_start }) = record.kind
            else {
                return false;
            };
            PhysRange {
                start: physical_start,
                end: physical_start.saturating_add((record.range.end - record.range.start) as u64),
            }
            .overlaps(physical)
        }) {
            return Err(MemoryError::InUse);
        }
        let range = state
            .mappings
            .gap(self.bounds, length, PAGE_SIZE)
            .ok_or(MemoryError::NoMemory)?;
        let generation = state.next_generation()?;
        state.mappings.insert(MappingRecord {
            range,
            maximum: MappingRights::READ | MappingRights::WRITE,
            generation,
            kind: MappingKind::Device {
                physical_start: physical.start,
            },
            journal: MappingJournal::default(),
        })?;
        self.table.edit(|table| {
            let mut attempted_end = range.start;
            let result: Result<(), MemoryError> = (|| {
                for address in pages(range) {
                    attempted_end = address + PAGE_SIZE;
                    let walk = table.prepare(address)?;
                    PageTableState::write_leaf(
                        address,
                        walk.tables[0].unwrap(),
                        Some(PageMapping {
                            physical_address: physical.start + (address - range.start) as u64,
                            attributes,
                        }),
                    )?;
                }
                Ok(())
            })();
            mmu.invalidate_all();
            if result.is_err() {
                // 回滚失败时保留记录，避免残余映射被重新分配或产生别名。
                clear_device_pages(
                    table,
                    VirtRange {
                        start: range.start,
                        end: attempted_end,
                    },
                )?;
                state.mappings.remove(range.start);
            }
            result
        })?;
        Ok((range, generation))
    }

    pub(super) fn unmap_device(&self, mapping: &DeviceMapping) -> Result<(), MemoryError> {
        let mut state = self.state.lock();
        let record = state
            .mappings
            .get(mapping.pages.start)
            .ok_or(MemoryError::NotMapped)?;
        if !matches!(record.kind, MappingKind::Device { .. })
            || record.range != mapping.pages
            || record.generation != mapping.generation
        {
            return Err(MemoryError::InvalidRequest);
        }
        self.table
            .edit(|table| clear_device_pages(table, record.range))?;
        state.mappings.remove(record.range.start);
        Ok(())
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
            kind: MappingKind::Permanent {
                physical_start: mapping.physical_address,
            },
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
        owner: Option<u64>,
    ) -> Result<(), MemoryError> {
        for address in pages(range) {
            let mut record = self.mappings.get(address).ok_or(MemoryError::NotMapped)?;
            let owned = owner.is_some_and(|generation| {
                record.kind == MappingKind::Owned && record.generation == generation
            });
            if !(owned || owner.is_none() && record.kind == MappingKind::Mapped) {
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

    fn withdraw_entries(&mut self, range: VirtRange) -> Result<(), MemoryError> {
        for address in pages(range) {
            let mut record = self.record(address);
            PageTableState::write_leaf(address, record.journal.tables[0].unwrap(), None)?;
            record.journal.written = true;
            self.mappings.set(record);
        }
        hardware::mmu().invalidate_all();
        Ok(())
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
        }
        let allocator = manager().allocator().lock();
        for address in pages(range).rev() {
            if let Some(record) = self.mappings.get(address)
                && record.journal.referenced
                && FrameAccess::from(record.journal.permissions()) != FrameAccess::Read
            {
                manager().restore_direct_access(
                    &allocator,
                    record.journal.frame(),
                    record.journal.direct_was_writable,
                );
            }
        }
        drop(allocator);
        for address in pages(range) {
            if self.mappings.get(address).is_none() {
                break;
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
        if FrameAccess::from(permissions) != FrameAccess::Read {
            for address in pages(range).rev() {
                let journal = self.record(address).journal;
                if journal.referenced {
                    manager().restore_direct_access(
                        allocator,
                        journal.frame(),
                        journal.direct_was_writable,
                    );
                }
            }
        }
        for address in pages(range) {
            let journal = self.record(address).journal;
            let frame = journal.frame();
            let head = allocator
                .allocation_of(frame)
                .expect("lost mapping allocation");
            manager()
                .reserve_access(
                    allocator,
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

fn clear_device_pages(table: &mut PageTableState, range: VirtRange) -> Result<(), MemoryError> {
    let result: Result<(), MemoryError> = (|| {
        for address in pages(range) {
            let walk = table.walk(address)?;
            if matches!(walk.entry, PageTableEntry::Mapping(_)) {
                PageTableState::write_leaf(address, walk.tables[0].unwrap(), None)?;
            }
        }
        Ok(())
    })();
    hardware::mmu().invalidate_all();
    result?;
    let block_bits = PAGE_SIZE.trailing_zeros() + hardware::mmu().layout().index_bits;
    let mut previous = None;
    for address in pages(range) {
        let block = address >> block_bits;
        if previous != Some(block) {
            table.prune_empty(address)?;
            previous = Some(block);
        }
    }
    Ok(())
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
