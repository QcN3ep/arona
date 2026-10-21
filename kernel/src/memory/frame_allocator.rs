// SPDX-License-Identifier: MPL-2.0

use super::{
    Frame, FrameAccess, FrameAllocationInfo, FrameOwner, FramePool, FramePurpose,
    MAX_MEMORY_REGIONS, MemoryError, MemoryRegion, MemoryRegionState, PhysRange, ReferenceKind,
};
use crate::hardware::{self, mmu::PAGE_SIZE};

pub(super) const KERNEL_POOL_FRAMES: u64 = 32 * 1024 * 1024 / PAGE_SIZE as u64;

const END: u64 = u64::MAX;
const WORDS: usize = 12;
const REGION_WORDS: usize = 5;
const REGIONS_PER_FRAME: usize = PAGE_SIZE / (REGION_WORDS * size_of::<u64>());
const DIRECTORY_FRAMES: usize = MAX_MEMORY_REGIONS.div_ceil(REGIONS_PER_FRAME);
const RECORDS_PER_FRAME: u64 = (PAGE_SIZE / (WORDS * size_of::<u64>())) as u64;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PoolStats {
    pub total_frames: u64,
    pub free_frames: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MemoryStats {
    pub kernel: PoolStats,
    pub user: PoolStats,
    pub reserved_frames: u64,
    pub metadata_frames: u64,
}

pub(super) struct FrameAllocator {
    metadata: FrameMetadata,
    free: [u64; 2],
    pools: [PoolStats; 2],
    reserved_frames: u64,
}

#[derive(Clone, Copy)]
struct ManagedRegion {
    range: PhysRange,
    state: MemoryRegionState,
    first: u64,
    available_first: u64,
}

struct FrameMetadata {
    directory: [u64; DIRECTORY_FRAMES],
    region_count: usize,
    directory_frames: u64,
    frames: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FrameState {
    Free,
    Owned,
    Referenced,
    Reserved,
    Metadata,
}

#[derive(Clone, Copy)]
struct FrameRecord {
    next: u64,
    allocation: u64,
    owner: u64,
    state: FrameState,
    pool: FramePool,
    purpose: FramePurpose,
    frames: u64,
    references: [u64; 3],
    writers: [u64; 3],
    executors: u64,
}

impl FrameAllocator {
    pub fn new(regions: &[MemoryRegion]) -> Result<Self, MemoryError> {
        let metadata = FrameMetadata::new(regions)?;
        let mut allocator = Self {
            metadata,
            free: [END; 2],
            pools: [PoolStats::default(); 2],
            reserved_frames: 0,
        };
        let mut kernel_frames = KERNEL_POOL_FRAMES;
        for region in allocator.metadata.regions() {
            let metadata_frames = allocator
                .metadata
                .frames
                .saturating_sub(region.available_first)
                .min(region.frame_count());
            for offset in 0..region.frame_count() {
                let index = region.first + offset;
                let record = if region.state == MemoryRegionState::BootReserved {
                    allocator.reserved_frames += 1;
                    FrameRecord::new(FrameState::Reserved, FramePool::Kernel)
                } else if offset < metadata_frames {
                    FrameRecord::new(FrameState::Metadata, FramePool::Kernel)
                } else {
                    let pool = if kernel_frames != 0 {
                        kernel_frames -= 1;
                        FramePool::Kernel
                    } else {
                        FramePool::User
                    };
                    allocator.pools[pool as usize].total_frames += 1;
                    let mut record = FrameRecord::new(FrameState::Free, pool);
                    record.next = allocator.free[pool as usize];
                    allocator.free[pool as usize] = index;
                    allocator.pools[pool as usize].free_frames += 1;
                    record
                };
                allocator.metadata.write(index, record);
            }
        }
        Ok(allocator)
    }

    pub fn stats(&self) -> MemoryStats {
        MemoryStats {
            kernel: self.pools[FramePool::Kernel as usize],
            user: self.pools[FramePool::User as usize],
            reserved_frames: self.reserved_frames,
            metadata_frames: self.metadata.frames,
        }
    }

    pub fn allocate(
        &mut self,
        owner: FrameOwner,
        purpose: FramePurpose,
        frames: u64,
    ) -> Result<u64, MemoryError> {
        if frames == 0
            || frames.checked_mul(PAGE_SIZE as u64).is_none()
            || (purpose == FramePurpose::PageTable && owner != FrameOwner::Kernel)
        {
            return Err(MemoryError::InvalidRequest);
        }
        let pool = owner.pool();
        if frames > self.pools[pool as usize].free_frames {
            return Err(MemoryError::NoMemory);
        }
        let mut head = END;
        for _ in 0..frames {
            let index = self.free[pool as usize];
            let mut record = self.metadata.read(index);
            self.free[pool as usize] = record.next;
            self.pools[pool as usize].free_frames -= 1;
            record.next = head;
            record.state = FrameState::Owned;
            record.owner = match owner {
                FrameOwner::Kernel => 0,
                FrameOwner::User(id) => id,
            };
            record.purpose = purpose;
            self.metadata.write(index, record);
            head = index;
        }
        let mut record = self.metadata.read(head);
        record.frames = frames;
        self.metadata.write(head, record);
        let mut index = head;
        while index != END {
            let mut record = self.metadata.read(index);
            record.allocation = head;
            self.metadata.write(index, record);
            index = record.next;
        }
        Ok(head)
    }

    pub fn info(&self, head: u64) -> FrameAllocationInfo {
        let record = self.metadata.read(head);
        FrameAllocationInfo {
            owner: match record.pool {
                FramePool::Kernel => FrameOwner::Kernel,
                FramePool::User => FrameOwner::User(record.owner),
            },
            purpose: record.purpose,
            frames: record.frames,
            mappings: record.references[ReferenceKind::Mapping as usize],
            dma: record.references[ReferenceKind::Dma as usize],
            temporary: record.references[ReferenceKind::Temporary as usize],
        }
    }

    pub fn frame(&self, index: u64) -> (u64, u64) {
        (
            self.metadata.physical_address(index),
            self.metadata.read(index).next,
        )
    }

    pub fn reference(&mut self, head: u64, kind: ReferenceKind) -> Result<(), MemoryError> {
        let mut record = self.metadata.read(head);
        let count = &mut record.references[kind as usize];
        *count = count.checked_add(1).ok_or(MemoryError::ReferenceOverflow)?;
        self.metadata.write(head, record);
        Ok(())
    }

    pub fn reserve_access(
        &mut self,
        head: u64,
        frame: Frame,
        kind: ReferenceKind,
        access: FrameAccess,
    ) -> Result<(), MemoryError> {
        let index = self.metadata.index(frame.address())?;
        let mut record = self.metadata.read(index);
        if record.allocation_head()? != head {
            return Err(MemoryError::InvalidFrame);
        }
        if access == FrameAccess::Read {
            return Ok(());
        }
        record.reserve_access(kind, access)?;
        self.metadata.write(index, record);
        Ok(())
    }

    pub fn release_access(&mut self, frame: Frame, kind: ReferenceKind, access: FrameAccess) {
        if access == FrameAccess::Read {
            return;
        }
        let index = self
            .metadata
            .index(frame.address())
            .expect("unmanaged frame access");
        let mut record = self.metadata.read(index);
        record.release_access(kind, access);
        self.metadata.write(index, record);
    }

    pub fn release_owner(&mut self, head: u64) {
        let mut record = self.metadata.read(head);
        assert_eq!(
            record.state,
            FrameState::Owned,
            "frame owner was already released"
        );
        if record.is_referenced() {
            record.state = FrameState::Referenced;
            self.metadata.write(head, record);
        } else {
            self.recycle(head);
        }
    }

    pub fn release_reference(&mut self, head: u64, kind: ReferenceKind) {
        let mut record = self.metadata.read(head);
        record.references[kind as usize] = record.references[kind as usize]
            .checked_sub(1)
            .expect("frame reference count underflow");
        self.metadata.write(head, record);
        if record.state == FrameState::Referenced && !record.is_referenced() {
            self.recycle(head);
        }
    }

    fn recycle(&mut self, mut index: u64) {
        while index != END {
            let record = self.metadata.read(index);
            let pool = record.pool as usize;
            let mut free = FrameRecord::new(FrameState::Free, record.pool);
            free.next = self.free[pool];
            self.metadata.write(index, free);
            self.free[pool] = index;
            self.pools[pool].free_frames += 1;
            index = record.next;
        }
    }

    pub fn allocation_of(&self, frame: Frame) -> Result<u64, MemoryError> {
        let index = self.metadata.index(frame.address())?;
        self.metadata.read(index).allocation_head()
    }

    pub fn is_managed(&self, address: u64) -> bool {
        self.metadata.index(address).is_ok()
    }

    pub fn merge(&mut self, head: u64, other: u64) {
        let mut record = self.metadata.read(head);
        let source = self.metadata.read(other);
        assert!(
            head != other
                && record.state == FrameState::Owned
                && source.state == FrameState::Owned
                && record.owner == source.owner
                && record.pool == source.pool
                && record.purpose == source.purpose
                && !record.is_referenced()
                && !source.is_referenced(),
            "cannot merge these allocations"
        );
        let mut tail = other;
        loop {
            let mut last = self.metadata.read(tail);
            last.allocation = head;
            self.metadata.write(tail, last);
            if last.next == END {
                last.next = record.next;
                self.metadata.write(tail, last);
                break;
            }
            tail = last.next;
        }
        record.next = other;
        record.frames += source.frames;
        self.metadata.write(head, record);
    }

    pub fn detach(&mut self, head: u64, frame: Frame) -> u64 {
        let index = self
            .metadata
            .index(frame.address())
            .expect("unmanaged frame");
        let allocation = self.metadata.read(head);
        assert!(
            head != index && !allocation.is_referenced(),
            "cannot detach frame"
        );
        let mut previous = head;
        loop {
            let mut record = self.metadata.read(previous);
            assert_ne!(record.next, END, "frame is outside allocation");
            if record.next == index {
                let mut detached = self.metadata.read(index);
                record.next = detached.next;
                self.metadata.write(previous, record);
                let mut allocation = self.metadata.read(head);
                allocation.frames -= 1;
                self.metadata.write(head, allocation);
                detached.next = END;
                detached.allocation = index;
                detached.frames = 1;
                self.metadata.write(index, detached);
                return index;
            }
            previous = record.next;
        }
    }

    fn range_frames(range: PhysRange) -> Result<impl Iterator<Item = u64>, MemoryError> {
        if range.start >= range.end
            || !range.start.is_multiple_of(PAGE_SIZE as u64)
            || !range.end.is_multiple_of(PAGE_SIZE as u64)
        {
            return Err(MemoryError::InvalidRequest);
        }
        Ok((range.start..range.end).step_by(PAGE_SIZE))
    }

    pub fn borrow_reserved(&mut self, range: PhysRange) -> Result<(), MemoryError> {
        for address in Self::range_frames(range)? {
            let index = self.metadata.index(address)?;
            let record = self.metadata.read(index);
            if record.state != FrameState::Reserved {
                return Err(MemoryError::InvalidFrame);
            }
            if record.references[ReferenceKind::Temporary as usize] == u64::MAX {
                return Err(MemoryError::ReferenceOverflow);
            }
        }
        for address in Self::range_frames(range)? {
            let index = self.metadata.index(address)?;
            self.reference(index, ReferenceKind::Temporary)?;
        }
        Ok(())
    }

    pub fn release_reserved_reference(&mut self, range: PhysRange) {
        for address in Self::range_frames(range).expect("invalid reserved frame reference") {
            let index = self
                .metadata
                .index(address)
                .expect("reserved frame is missing");
            self.release_reference(index, ReferenceKind::Temporary);
        }
    }

    pub fn reclaim_reserved(
        &mut self,
        range: PhysRange,
        pool: FramePool,
    ) -> Result<(), MemoryError> {
        for address in Self::range_frames(range)? {
            let index = self.metadata.index(address)?;
            let record = self.metadata.read(index);
            if record.state != FrameState::Reserved {
                return Err(MemoryError::InvalidFrame);
            }
            if record.is_referenced() {
                return Err(MemoryError::InUse);
            }
        }
        for address in Self::range_frames(range)? {
            let index = self.metadata.index(address)?;
            let mut record = FrameRecord::new(FrameState::Free, pool);
            record.next = self.free[pool as usize];
            self.metadata.write(index, record);
            self.free[pool as usize] = index;
            self.pools[pool as usize].total_frames += 1;
            self.pools[pool as usize].free_frames += 1;
            self.reserved_frames -= 1;
        }
        Ok(())
    }
}

impl FrameRecord {
    fn new(state: FrameState, pool: FramePool) -> Self {
        Self {
            next: END,
            allocation: END,
            owner: 0,
            state,
            pool,
            purpose: FramePurpose::Data,
            frames: 0,
            references: [0; 3],
            writers: [0; 3],
            executors: 0,
        }
    }

    fn is_referenced(self) -> bool {
        self.references.iter().any(|count| *count != 0)
    }

    fn allocation_head(self) -> Result<u64, MemoryError> {
        if matches!(self.state, FrameState::Owned | FrameState::Referenced) {
            Ok(self.allocation)
        } else {
            Err(MemoryError::InvalidFrame)
        }
    }

    fn reserve_access(
        &mut self,
        kind: ReferenceKind,
        access: FrameAccess,
    ) -> Result<(), MemoryError> {
        let count = match access {
            FrameAccess::Read => return Ok(()),
            FrameAccess::Write if self.executors == 0 => &mut self.writers[kind as usize],
            FrameAccess::Execute
                if kind == ReferenceKind::Mapping
                    && self.purpose == FramePurpose::Data
                    && self.writers == [0; 3] =>
            {
                &mut self.executors
            }
            _ => return Err(MemoryError::AccessConflict),
        };
        *count = count.checked_add(1).ok_or(MemoryError::ReferenceOverflow)?;
        Ok(())
    }

    fn release_access(&mut self, kind: ReferenceKind, access: FrameAccess) {
        let count = match access {
            FrameAccess::Read => return,
            FrameAccess::Write => &mut self.writers[kind as usize],
            FrameAccess::Execute => &mut self.executors,
        };
        *count = count.checked_sub(1).expect("frame access count underflow");
    }

    fn words(self) -> [u64; WORDS] {
        [
            self.next,
            self.owner,
            self.state as u64 | (self.pool as u64) << 8 | (self.purpose as u64) << 16,
            self.frames,
            self.references[0],
            self.references[1],
            self.references[2],
            self.allocation,
            self.writers[0],
            self.writers[1],
            self.writers[2],
            self.executors,
        ]
    }

    fn from_words(words: [u64; WORDS]) -> Self {
        let state = match words[2] & 0xff {
            0 => FrameState::Free,
            1 => FrameState::Owned,
            2 => FrameState::Referenced,
            3 => FrameState::Reserved,
            4 => FrameState::Metadata,
            _ => panic!("invalid frame state"),
        };
        let pool = match (words[2] >> 8) & 0xff {
            0 => FramePool::Kernel,
            1 => FramePool::User,
            _ => panic!("invalid frame pool"),
        };
        let purpose = match words[2] >> 16 {
            0 => FramePurpose::Data,
            1 => FramePurpose::PageTable,
            2 => FramePurpose::Stack,
            _ => panic!("invalid frame purpose"),
        };
        Self {
            next: words[0],
            allocation: words[7],
            owner: words[1],
            state,
            pool,
            purpose,
            frames: words[3],
            references: [words[4], words[5], words[6]],
            writers: [words[8], words[9], words[10]],
            executors: words[11],
        }
    }
}

impl FrameMetadata {
    fn new(regions: &[MemoryRegion]) -> Result<Self, MemoryError> {
        if regions.len() > MAX_MEMORY_REGIONS {
            return Err(MemoryError::InvalidRequest);
        }
        let mut previous_end = 0;
        for region in regions {
            let range = region.range;
            if range.start >= range.end
                || !range.start.is_multiple_of(PAGE_SIZE as u64)
                || !range.end.is_multiple_of(PAGE_SIZE as u64)
                || range.start < previous_end
            {
                return Err(MemoryError::InvalidRequest);
            }
            previous_end = range.end;
        }
        let mut region_count = 0usize;
        let mut count = 0u64;
        let mut available = 0u64;
        for region in Self::input_regions(regions) {
            region_count += 1;
            count = count
                .checked_add(region.frame_count())
                .ok_or(MemoryError::InvalidRequest)?;
            if region.state == MemoryRegionState::Available {
                available += region.frame_count();
            }
        }
        let directory_frames = region_count.div_ceil(REGIONS_PER_FRAME) as u64;
        let frames = directory_frames + count.div_ceil(RECORDS_PER_FRAME);
        if available.saturating_sub(frames) < KERNEL_POOL_FRAMES {
            return Err(MemoryError::NoMemory);
        }
        let mut metadata = Self {
            directory: [0; DIRECTORY_FRAMES],
            region_count,
            directory_frames,
            frames,
        };
        let mut remaining = frames;
        let mut first = 0;
        for region in Self::input_regions(regions) {
            if region.state != MemoryRegionState::Available {
                continue;
            }
            let prefix = remaining.min(region.frame_count());
            for offset in 0..prefix {
                let address = region.range.start + offset * PAGE_SIZE as u64;
                if first < directory_frames {
                    metadata.directory[first as usize] = address;
                }
                hardware::mmu().zero_frame(address)?;
                first += 1;
            }
            remaining -= prefix;
        }
        let mut first = 0;
        let mut available_first = 0;
        for (index, mut region) in Self::input_regions(regions).enumerate() {
            region.first = first;
            region.available_first = available_first;
            first += region.frame_count();
            if region.state == MemoryRegionState::Available {
                available_first += region.frame_count();
            }
            metadata.write_region(index, region)?;
        }
        Ok(metadata)
    }

    fn input_regions(regions: &[MemoryRegion]) -> impl Iterator<Item = ManagedRegion> + '_ {
        let limit = 1u64
            .checked_shl(hardware::mmu().physical_address_bits())
            .unwrap_or(!(PAGE_SIZE as u64 - 1));
        regions.iter().filter_map(move |region| {
            if matches!(
                region.state,
                MemoryRegionState::Reserved | MemoryRegionState::NoMap
            ) {
                return None;
            }
            let range = PhysRange {
                start: region.range.start,
                end: region.range.end.min(limit),
            };
            (range.start < range.end).then_some(ManagedRegion {
                range,
                state: region.state,
                first: 0,
                available_first: 0,
            })
        })
    }

    fn write_region(&self, index: usize, region: ManagedRegion) -> Result<(), MemoryError> {
        hardware::mmu().write_frame(
            self.directory[index / REGIONS_PER_FRAME],
            (index % REGIONS_PER_FRAME) * REGION_WORDS,
            &[
                region.range.start,
                region.range.end,
                region.first,
                u64::from(region.state == MemoryRegionState::BootReserved),
                region.available_first,
            ],
        )?;
        Ok(())
    }

    fn regions(&self) -> impl Iterator<Item = ManagedRegion> + '_ {
        (0..self.region_count).map(|index| self.region(index))
    }

    fn region(&self, index: usize) -> ManagedRegion {
        let mut words = [0; REGION_WORDS];
        hardware::mmu()
            .read_frame(
                self.directory[index / REGIONS_PER_FRAME],
                (index % REGIONS_PER_FRAME) * REGION_WORDS,
                &mut words,
            )
            .expect("failed to read memory region");
        ManagedRegion {
            range: PhysRange {
                start: words[0],
                end: words[1],
            },
            first: words[2],
            available_first: words[4],
            state: if words[3] == 0 {
                MemoryRegionState::Available
            } else {
                MemoryRegionState::BootReserved
            },
        }
    }

    fn find_region(&self, before: impl Fn(ManagedRegion) -> bool) -> Option<ManagedRegion> {
        let mut start = 0;
        let mut end = self.region_count;
        while start < end {
            let middle = start + (end - start) / 2;
            let region = self.region(middle);
            if before(region) {
                start = middle + 1;
            } else {
                end = middle;
            }
        }
        (start < self.region_count).then(|| self.region(start))
    }

    fn index(&self, address: u64) -> Result<u64, MemoryError> {
        if !address.is_multiple_of(PAGE_SIZE as u64) {
            return Err(MemoryError::InvalidFrame);
        }
        self.find_region(|region| region.range.end <= address)
            .filter(|region| region.range.start <= address)
            .map(|region| region.first + (address - region.range.start) / PAGE_SIZE as u64)
            .ok_or(MemoryError::InvalidFrame)
    }

    fn physical_address(&self, index: u64) -> u64 {
        let region = self
            .find_region(|region| region.first + region.frame_count() <= index)
            .expect("frame index is outside managed memory");
        region.range.start + (index - region.first) * PAGE_SIZE as u64
    }

    fn location(&self, index: u64) -> (u64, usize) {
        let frame = self.directory_frames + index / RECORDS_PER_FRAME;
        let region = self
            .find_region(|region| {
                let available = if region.state == MemoryRegionState::Available {
                    region.frame_count()
                } else {
                    0
                };
                region.available_first + available <= frame
            })
            .expect("frame metadata is missing");
        (
            region.range.start + (frame - region.available_first) * PAGE_SIZE as u64,
            (index % RECORDS_PER_FRAME) as usize * WORDS,
        )
    }

    fn read(&self, index: u64) -> FrameRecord {
        let (address, offset) = self.location(index);
        let mut words = [0; WORDS];
        hardware::mmu()
            .read_frame(address, offset, &mut words)
            .expect("failed to read frame metadata");
        FrameRecord::from_words(words)
    }

    fn write(&self, index: u64, record: FrameRecord) {
        let (address, offset) = self.location(index);
        hardware::mmu()
            .write_frame(address, offset, &record.words())
            .expect("failed to write frame metadata");
    }
}

impl ManagedRegion {
    fn frame_count(&self) -> u64 {
        (self.range.end - self.range.start) / PAGE_SIZE as u64
    }
}
