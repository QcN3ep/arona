// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use core::{
    ops::Range,
    sync::atomic::{AtomicUsize, Ordering},
};
use spin::Mutex;

use super::{
    Frame, FrameAccess, FrameAllocation, FrameAllocationReference, HUGE_PAGE_FRAMES,
    HUGE_PAGE_SIZE, MemoryError, PhysRange, ReferenceKind, VirtRange,
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
/// Operations and drop must satisfy the execution environment requirements of `Mmu`; all processors must switch away from the address space before drop.
pub struct AddressSpace {
    table: PageTable,
    state: Mutex<AddressSpaceState>,
    bounds: VirtRange,
}

static ADDRESS_SPACES: AtomicUsize = AtomicUsize::new(0);

pub fn allocated_address_spaces() -> usize {
    ADDRESS_SPACES.load(Ordering::Relaxed)
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
        ADDRESS_SPACES.fetch_add(1, Ordering::Relaxed);
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

    pub(super) fn map_stack(
        &self,
        allocation: &FrameAllocation,
    ) -> Result<(VirtRange, OwnedMapping), MemoryError> {
        if self.table.is_user() {
            return Err(MemoryError::InvalidRequest);
        }
        let reference = allocation.reference(ReferenceKind::Mapping)?;
        let frames = 0..reference.info().frames;
        let options = MappingOptions::new(PagePermissions::ReadWrite);
        let length = self.validate_source(&reference, &frames, options)?;
        let mut state = self.state.lock();
        let full = state
            .mappings
            .gap(
                self.bounds,
                length.checked_add(PAGE_SIZE).ok_or(MemoryError::NoMemory)?,
                PAGE_SIZE,
            )
            .ok_or(MemoryError::NoMemory)?;
        let guard = VirtRange {
            start: full.start,
            end: full.start + PAGE_SIZE,
        };
        let generation = state.next_generation()?;
        state.mappings.insert(MappingRecord {
            range: guard,
            generation,
            maximum: MappingRights(0),
            kind: MappingKind::Guard,
            journal: MappingJournal::default(),
        })?;
        let range = VirtRange {
            start: guard.end,
            end: full.end,
        };
        if let Err(error) = self.map_locked(
            &mut state,
            range,
            &reference,
            frames,
            options,
            MappingKind::Owned,
        ) {
            state.mappings.remove(guard.start);
            return Err(error);
        }
        Ok((
            guard,
            OwnedMapping {
                range,
                generation: state.record(range.start).generation,
            },
        ))
    }

    pub(super) fn unmap_stack(
        &self,
        guard: VirtRange,
        mapping: &OwnedMapping,
    ) -> Result<(), MemoryError> {
        self.unmap_owned(mapping)?;
        self.state.lock().mappings.remove(guard.start);
        Ok(())
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

    /// Maps `frames` as large leaves into the first suitably aligned gap.
    pub fn map_huge(
        &self,
        allocation: &FrameAllocation,
        frames: Range<u64>,
        options: MappingOptions,
    ) -> Result<VirtRange, MemoryError> {
        self.map_huge_reference(
            allocation.reference(ReferenceKind::Mapping)?,
            frames,
            None,
            options,
        )
    }

    /// Maps `frames` as large leaves at the requested address.
    pub fn map_huge_at(
        &self,
        address: usize,
        allocation: &FrameAllocation,
        frames: Range<u64>,
        options: MappingOptions,
    ) -> Result<VirtRange, MemoryError> {
        self.map_huge_reference(
            allocation.reference(ReferenceKind::Mapping)?,
            frames,
            Some(address),
            options,
        )
    }

    fn map_huge_reference(
        &self,
        reference: FrameAllocationReference,
        frames: Range<u64>,
        address: Option<usize>,
        options: MappingOptions,
    ) -> Result<VirtRange, MemoryError> {
        let count = frames
            .end
            .checked_sub(frames.start)
            .ok_or(MemoryError::InvalidRequest)?;
        if frames.start >= frames.end
            || !frames.start.is_multiple_of(HUGE_PAGE_FRAMES)
            || !count.is_multiple_of(HUGE_PAGE_FRAMES)
        {
            return Err(MemoryError::InvalidRequest);
        }
        let length = self.validate_source(&reference, &frames, options)?;
        if !length.is_multiple_of(HUGE_PAGE_SIZE) {
            return Err(MemoryError::InvalidRequest);
        }
        self.validate_huge_source(&reference, &frames)?;
        let mut state = self.state.lock();
        let range = match address {
            Some(address) => {
                if !address.is_multiple_of(HUGE_PAGE_SIZE) {
                    return Err(MemoryError::InvalidRequest);
                }
                let range = VirtRange {
                    start: address,
                    end: address
                        .checked_add(length)
                        .ok_or(MemoryError::InvalidRequest)?,
                };
                self.validate_range(range)?;
                range
            }
            None => state
                .mappings
                .gap(self.bounds, length, HUGE_PAGE_SIZE)
                .ok_or(MemoryError::NoMemory)?,
        };
        self.map_leaves_locked(
            &mut state,
            range,
            &reference,
            frames,
            options,
            LeafLayout {
                level: 1,
                kind: MappingKind::Huge,
            },
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

    /// Checks that `frames` covers one physically contiguous, large-leaf aligned range.
    ///
    /// A large leaf maps `base .. base + leaf size` directly, so the underlying
    /// allocation must cover exactly those frames in physical order. Rejecting the
    /// request here keeps a later per-frame reservation failure from leaving the
    /// allocation head referenced.
    fn validate_huge_source(
        &self,
        reference: &FrameAllocationReference,
        frames: &Range<u64>,
    ) -> Result<(), MemoryError> {
        let start = usize::try_from(frames.start).map_err(|_| MemoryError::InvalidRequest)?;
        let count =
            usize::try_from(frames.end - frames.start).map_err(|_| MemoryError::InvalidRequest)?;
        let mut addresses = reference.frames().skip(start);
        let base = addresses.next().ok_or(MemoryError::InvalidFrame)?.address();
        if !base.is_multiple_of(HUGE_PAGE_SIZE as u64) {
            return Err(MemoryError::InvalidRequest);
        }
        let mut expected = base;
        for frame in addresses.take(count - 1) {
            expected += PAGE_SIZE as u64;
            if frame.address() != expected {
                return Err(MemoryError::InvalidRequest);
            }
        }
        Ok(())
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
        self.map_leaves_locked(
            state,
            range,
            reference,
            frames,
            options,
            LeafLayout { level: 0, kind },
        )
    }

    /// Installs one mapping record per leaf of `leaf_level`, so base-page and large
    /// leaf mappings share preparation, reference accounting, rollback, and teardown.
    fn map_leaves_locked(
        &self,
        state: &mut AddressSpaceState,
        range: VirtRange,
        reference: &FrameAllocationReference,
        frames: Range<u64>,
        options: MappingOptions,
        leaf: LeafLayout,
    ) -> Result<(), MemoryError> {
        let leaf_level = leaf.level;
        let leaf_size = leaf_size(leaf_level);
        let frames_per_leaf = leaf_size / PAGE_SIZE;
        let length = range.end - range.start;
        if length == 0
            || !length.is_multiple_of(leaf_size)
            || frames.end - frames.start != (length / PAGE_SIZE) as u64
        {
            return Err(MemoryError::InvalidRequest);
        }
        if state.mappings.overlaps(range) {
            return Err(MemoryError::AlreadyMapped);
        }
        let user = self.table.is_user();
        let records = length / leaf_size;
        let mut quota = manager().reserve_mapping_records(if user { records } else { 0 })?;
        let generation = state.next_generation()?;
        let attributes = PageAttributes {
            permissions: options.permissions,
            memory_type: MemoryType::Normal,
            user_accessible: user,
        };
        let access_kind = FrameAccess::from(options.permissions);
        // Walk the allocation once. Rebuilding `frames()` and calling `nth` for
        // every leaf made base-page mappings quadratic in the page count because
        // each step locks the frame allocator.
        let start = usize::try_from(frames.start).map_err(|_| MemoryError::InvalidRequest)?;
        let mut source = reference.frames().skip(start);
        let result = (|| {
            self.table.edit(|table| {
                for address in (range.start..range.end).step_by(leaf_size) {
                    let frame = if address == range.start {
                        source.next()
                    } else {
                        source.nth(frames_per_leaf - 1)
                    }
                    .ok_or(MemoryError::InvalidFrame)?;
                    let mut record = MappingRecord {
                        range: VirtRange {
                            start: address,
                            end: address + leaf_size,
                        },
                        maximum: options.maximum,
                        generation,
                        kind: MappingKind::Preparing,
                        journal: MappingJournal {
                            mapping: Some(PageMapping {
                                physical_address: frame.address(),
                                attributes,
                            }),
                            ..MappingJournal::default()
                        },
                    };
                    state.mappings.insert(record)?;
                    record.journal.tables = table.prepare_leaf(address, leaf_level)?.tables;
                    state.mappings.set(record);
                }
                Ok::<_, MemoryError>(())
            })?;
            for address in (range.start..range.end).step_by(leaf_size) {
                let mut record = state.record(address);
                let mut reserved = 0;
                let result = (|| {
                    for index in 0..frames_per_leaf {
                        let frame = record.frame_at(address + index * PAGE_SIZE);
                        let access = reference
                            .reference(ReferenceKind::Mapping)?
                            .access(frame, access_kind)?;
                        record.journal.direct_was_writable = access.direct_was_writable;
                        core::mem::forget(access);
                        reserved += 1;
                    }
                    Ok::<(), MemoryError>(())
                })();
                record.journal.reserved = reserved;
                if result.is_ok() {
                    record.journal.referenced = true;
                }
                state.mappings.set(record);
                result?;
            }
            for address in (range.start..range.end).step_by(leaf_size) {
                let mut record = state.record(address);
                PageTableState::write_leaf_at(
                    address,
                    leaf_level,
                    record.journal.tables[leaf_level].ok_or(MemoryError::InvalidRequest)?,
                    record.journal.mapping,
                )?;
                record.journal.written = true;
                state.mappings.set(record);
            }
            Ok::<_, MemoryError>(())
        })();
        if result.is_err() {
            self.table
                .edit(|table| state.rollback_map(table, range, leaf_level))?;
            return result;
        }
        hardware::mmu().invalidate_all();
        for address in (range.start..range.end).step_by(leaf_size) {
            let mut record = state.record(address);
            record.kind = leaf.kind;
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
        let mut cursor = range.start;
        while cursor < range.end {
            let record = state.mappings.get(cursor).ok_or(MemoryError::NotMapped)?;
            if record.range.start < range.start || record.range.end > range.end {
                return Err(MemoryError::InUse);
            }
            cursor = record.range.end;
        }
        let staged = self
            .table
            .edit(|table| state.stage(table, range, None, owner));
        if let Err(error) = staged {
            state.clear_journals(range);
            return Err(error);
        }
        let result = self.table.edit(|table| {
            let mut cursor = range.start;
            while cursor < range.end {
                let mut record = state.record(cursor);
                let end = record.range.end;
                write_record_leaf(cursor, &record, None)?;
                record.journal.written = true;
                state.mappings.set(record);
                cursor = end;
            }
            let mut cursor = range.start;
            while cursor < range.end {
                let mut record = state.record(cursor);
                let tables = record.journal.tables;
                let end = record.range.end;
                table.detach_empty(cursor, &tables, |level| {
                    record.journal.detached |= 1 << level;
                    state.mappings.set(record);
                })?;
                cursor = end;
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
            let mut cursor = range.start;
            while cursor < range.end {
                let record = state.record(cursor);
                table.release_tables(&record.journal.tables, record.journal.detached);
                for address in (record.range.start..record.range.end).step_by(PAGE_SIZE) {
                    manager()
                        .release_mapping(
                            record.frame_at(address),
                            record.journal.permissions().into(),
                        )
                        .expect("lost mapping allocation");
                }
                cursor = record.range.end;
            }
        });
        let mut records = 0;
        let mut cursor = range.start;
        while cursor < range.end {
            let record = state.record(cursor);
            let end = record.range.end;
            state.mappings.remove(record.range.start);
            records += 1;
            cursor = end;
        }
        if self.table.is_user() {
            manager().release_mapping_records(records);
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
        let mut cursor = range.start;
        while cursor < range.end {
            let record = state.mappings.get(cursor).ok_or(MemoryError::NotMapped)?;
            if record.kind == MappingKind::Huge {
                return Err(MemoryError::Unsupported);
            }
            cursor = record.range.end;
        }
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
        // Multiple aliases of the same page frame must revoke the old access counts together so that their permissions can be switched together.
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
                state.update_record(address, |record| {
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
                    Ok::<_, MemoryError>(())
                })?;
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
                state.update_record(address, |record| record.generation = generation);
            }
        }
        state.clear_journals(range);
        result
    }

    pub(crate) fn bounds(&self) -> VirtRange {
        self.bounds
    }

    pub fn query(&self, address: usize) -> Result<Option<MappingInfo>, MemoryError> {
        self.query_inner(address, false)
    }

    pub fn query_next(&self, address: usize) -> Result<Option<MappingInfo>, MemoryError> {
        self.query_inner(address, true)
    }

    fn query_inner(&self, address: usize, next: bool) -> Result<Option<MappingInfo>, MemoryError> {
        if address < self.bounds.start || address >= self.bounds.end {
            return Err(MemoryError::InvalidRequest);
        }
        super::paging::validate_address(address, false)?;
        let state = self.state.lock();
        let Some((mut info, mapped)) = state.mappings.query(address, next) else {
            return Ok(None);
        };
        if mapped {
            info.mapping = self.table.query(address.max(info.range.start))?;
        }
        Ok(Some(info))
    }

    pub(crate) fn pin_user_memory(
        &self,
        address: usize,
        length: usize,
        write: bool,
    ) -> Result<super::MemoryBuffer, super::UserMemoryError> {
        if length == 0 {
            return super::MemoryBuffer::new(0, 0).map_err(super::UserMemoryError);
        }
        let pin = || -> Result<super::MemoryBuffer, MemoryError> {
            let end = address
                .checked_add(length)
                .ok_or(MemoryError::InvalidRequest)?;
            if !self.table.is_user() || address < self.bounds.start || end > self.bounds.end {
                return Err(MemoryError::InvalidRequest);
            }
            let mut buffer = super::MemoryBuffer::new(address % PAGE_SIZE, length)?;
            let state = self.state.lock();
            for page in (address & !(PAGE_SIZE - 1)..end).step_by(PAGE_SIZE) {
                let record = state.mappings.get(page).ok_or(MemoryError::NotMapped)?;
                if !matches!(record.kind, MappingKind::Mapped | MappingKind::Huge) {
                    return Err(MemoryError::InvalidRequest);
                }
                let mapping = self.table.query(page)?.ok_or(MemoryError::NotMapped)?;
                let permissions = mapping.attributes.permissions;
                if !mapping.attributes.user_accessible
                    || mapping.attributes.memory_type != MemoryType::Normal
                    || (write && permissions != PagePermissions::ReadWrite)
                    || (!write
                        && matches!(
                            permissions,
                            PagePermissions::NoAccess | PagePermissions::ExecuteOnly
                        ))
                {
                    return Err(MemoryError::AccessConflict);
                }
                let frame = Frame::new(
                    mapping.physical_address
                        + u64::try_from(page - record.range.start)
                            .map_err(|_| MemoryError::InvalidRequest)?,
                )
                .ok_or(MemoryError::InvalidFrame)?;
                let head = manager().allocator().lock().allocation_of(frame)?;
                buffer.push(
                    manager()
                        .reference(head, ReferenceKind::Temporary)?
                        .access(
                            frame,
                            if write {
                                FrameAccess::Write
                            } else {
                                FrameAccess::Read
                            },
                        )?,
                );
            }
            Ok(buffer)
        };
        pin().map_err(super::UserMemoryError)
    }

    pub fn translate(&self, address: usize) -> Result<Option<u64>, MemoryError> {
        Ok(self.query(address)?.and_then(|info| {
            info.mapping
                .map(|mapping| mapping.physical_address + (address - info.range.start) as u64)
        }))
    }

    pub(super) fn map_device(&self, physical: PhysRange) -> Result<(VirtRange, u64), MemoryError> {
        let mmu = hardware::mmu();
        let bits = mmu.physical_address_bits();
        if physical.start >= physical.end
            || !physical.start.is_multiple_of(PAGE_SIZE as u64)
            || !physical.end.is_multiple_of(PAGE_SIZE as u64)
            || bits < 64 && physical.end > 1u64 << bits
        {
            return Err(MemoryError::InvalidRequest);
        }
        let length = usize::try_from(physical.end - physical.start)
            .map_err(|_| MemoryError::InvalidRequest)?;
        let attributes = PageAttributes {
            permissions: PagePermissions::ReadWrite,
            memory_type: mmu.device_memory_type(),
            user_accessible: self.table.is_user(),
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
        let user = self.table.is_user();
        let mut quota = manager().reserve_mapping_records(usize::from(user))?;
        state.mappings.insert(MappingRecord {
            range,
            maximum: MappingRights::READ | MappingRights::WRITE,
            generation,
            kind: MappingKind::Device {
                physical_start: physical.start,
            },
            journal: MappingJournal::default(),
        })?;
        quota.0 = 0;
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
                if let Err(error) = clear_device_pages(
                    table,
                    VirtRange {
                        start: range.start,
                        end: attempted_end,
                    },
                ) {
                    // Residual device mappings must not outlive the grant.
                    log::error!("Failed to roll back device mapping: {error:?}");
                    hardware::cpu().halt();
                }
                state.mappings.remove(range.start);
                manager().release_mapping_records(usize::from(user));
            }
            result
        })?;
        Ok((range, generation))
    }

    pub(super) fn unmap_device(
        &self,
        pages: VirtRange,
        generation: u64,
    ) -> Result<(), MemoryError> {
        let mut state = self.state.lock();
        let record = state
            .mappings
            .get(pages.start)
            .ok_or(MemoryError::NotMapped)?;
        if !matches!(record.kind, MappingKind::Device { .. } | MappingKind::Guard)
            || record.range != pages
            || record.generation != generation
        {
            return Err(MemoryError::InvalidRequest);
        }
        self.table
            .edit(|table| clear_device_pages(table, record.range))?;
        state.mappings.remove(record.range.start);
        manager().release_mapping_records(usize::from(self.table.is_user()));
        Ok(())
    }

    pub(super) fn revoke_device(
        &self,
        pages: VirtRange,
        generation: u64,
    ) -> Result<(), MemoryError> {
        let mut state = self.state.lock();
        let mut record = state
            .mappings
            .get(pages.start)
            .ok_or(MemoryError::NotMapped)?;
        if !matches!(record.kind, MappingKind::Device { .. } | MappingKind::Guard)
            || record.range != pages
            || record.generation != generation
        {
            return Err(MemoryError::InvalidRequest);
        }
        self.table
            .edit(|table| clear_device_pages(table, record.range))?;
        // Keep the address reserved until the mapping holder releases it, preventing stale accesses from hitting a new mapping.
        record.kind = MappingKind::Guard;
        state.mappings.set(record);
        Ok(())
    }

    /// # Safety
    ///
    /// Backing storage must remain valid for the entire kernel runtime.
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

    fn update_record<R>(
        &mut self,
        address: usize,
        update: impl FnOnce(&mut MappingRecord) -> R,
    ) -> R {
        self.mappings
            .update(address, update)
            .expect("missing mapping record")
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
            let mapped = record.kind == MappingKind::Mapped
                || (record.kind == MappingKind::Huge && permissions.is_none());
            if !(owned || owner.is_none() && mapped) {
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
            let cleared = self.mappings.update(address, |record| {
                if record.journal.mapping.is_none() {
                    return false;
                }
                record.journal = MappingJournal::default();
                true
            });
            if cleared != Some(true) {
                break;
            }
        }
    }

    fn withdraw_entries(&mut self, range: VirtRange) -> Result<(), MemoryError> {
        for address in pages(range) {
            let mut record = self.record(address);
            write_record_leaf(address, &record, None)?;
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
                write_record_leaf(address, &record, record.journal.mapping)
                    .expect("failed to restore page mapping");
            }
        }
        hardware::mmu().invalidate_all();
    }

    fn rollback_map(
        &mut self,
        table: &mut PageTableState,
        range: VirtRange,
        leaf_level: usize,
    ) -> Result<(), MemoryError> {
        let leaf_size = leaf_size(leaf_level);
        for address in (range.start..range.end).step_by(leaf_size) {
            let Some(record) = self.mappings.get(address) else {
                break;
            };
            if record.journal.written {
                write_record_leaf(address, &record, None)?;
            }
        }
        hardware::mmu().invalidate_all();
        for address in (range.start..range.end).step_by(leaf_size) {
            let Some(record) = self.mappings.get(address) else {
                break;
            };
            if record.journal.reserved != 0 {
                let permissions = record.journal.permissions().into();
                for index in 0..record.journal.reserved {
                    manager().release_mapping(
                        record.frame_at(address + index as usize * PAGE_SIZE),
                        permissions,
                    )?;
                }
            }
        }
        let allocator = manager().allocator().lock();
        for address in (range.start..range.end).step_by(leaf_size).rev() {
            if let Some(record) = self.mappings.get(address)
                && record.journal.reserved != 0
                && FrameAccess::from(record.journal.permissions()) != FrameAccess::Read
            {
                for index in 0..record.journal.reserved {
                    manager().restore_direct_access(
                        &allocator,
                        record.frame_at(address + index as usize * PAGE_SIZE),
                        record.journal.direct_was_writable,
                    );
                }
            }
        }
        drop(allocator);
        for address in (range.start..range.end).step_by(leaf_size) {
            if self.mappings.get(address).is_none() {
                break;
            }
            table.prune_empty(address)?;
            self.mappings.remove(address);
        }
        Ok(())
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
        ADDRESS_SPACES.fetch_sub(1, Ordering::Relaxed);
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

#[derive(Clone, Copy)]
struct LeafLayout {
    level: usize,
    kind: MappingKind,
}

fn leaf_size(leaf_level: usize) -> usize {
    PAGE_SIZE << (hardware::mmu().layout().index_bits * leaf_level as u32)
}

fn pages(range: VirtRange) -> core::iter::StepBy<Range<usize>> {
    (range.start..range.end).step_by(PAGE_SIZE)
}

/// Writes or clears the leaf covering `address` at the staged page table level.
fn write_record_leaf(
    address: usize,
    record: &MappingRecord,
    mapping: Option<PageMapping>,
) -> Result<(), MemoryError> {
    let level = record.journal.level();
    let table = record
        .journal
        .tables
        .get(level)
        .copied()
        .flatten()
        .ok_or(MemoryError::InvalidRequest)?;
    PageTableState::write_leaf_at(address, level, table, mapping)
}

fn validate_alignment(alignment: usize) -> Result<(), MemoryError> {
    if alignment < PAGE_SIZE || !alignment.is_power_of_two() {
        return Err(MemoryError::InvalidRequest);
    }
    Ok(())
}
