// SPDX-License-Identifier: MPL-2.0

//! Kernel heap allocation, usage accounting, and free cache reclamation.

use core::alloc::Layout;
use spin::Mutex;

pub use allocator_api2::boxed::Box as HeapBox;
pub use triomphe::Arc as HeapArc;

use super::{
    Frame, FrameAccess, FrameAccessReference, FrameAllocation, FrameOwner, FramePurpose,
    MappingOptions, MemoryError, ReferenceKind, manager, mapping::OwnedMapping,
};
use crate::hardware::{
    self,
    mmu::{PAGE_SIZE, PagePermissions},
};

const NIL: u64 = u64::MAX;
const MIN_SIZE: usize = 16;
const CLASSES: usize = 8;
const BITMAP_WORDS: usize = PAGE_SIZE / MIN_SIZE / u64::BITS as usize;
const HEADER_WORDS: usize = 4;
const RECORD_WORDS: usize = 7 + BITMAP_WORDS;
const RECORDS: usize = (PAGE_SIZE / size_of::<u64>() - HEADER_WORDS) / RECORD_WORDS;
const FULL: u64 = (1 << RECORDS) - 1;

pub(super) struct HeapAllocator {
    state: Mutex<HeapAllocatorState>,
}

struct HeapAllocatorState {
    slabs: SlabAllocator,
    allocations: usize,
    requested_bytes: usize,
    allocated_bytes: usize,
    mapped_frames: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HeapStats {
    pub allocations: usize,
    pub requested_bytes: usize,
    pub allocated_bytes: usize,
    pub backing_frames: usize,
    pub metadata_frames: usize,
    pub cached_frames: usize,
}

/// Exclusive heap storage, released on drop.
#[must_use]
pub struct HeapAllocation {
    layout: Layout,
    storage: HeapStorage,
}

enum HeapStorage {
    Empty,
    Slab(SlabAllocation),
    Mapped(OwnedMapping),
}

struct SlabAllocator {
    partial: [u64; CLASSES],
    empty: [u64; CLASSES],
    free_records: u64,
    frames: usize,
    metadata_frames: usize,
}

struct SlabAllocation {
    address: usize,
    record: u64,
    _access: FrameAccessReference,
}

struct Slab {
    backing: u64,
    frame: Frame,
    address: usize,
    previous: u64,
    next: u64,
    class: usize,
    used: usize,
    bitmap: [u64; BITMAP_WORDS],
}

struct SlabMetadata {
    backing: u64,
    previous: u64,
    next: u64,
    occupied: u64,
}

impl HeapAllocator {
    pub const fn new() -> Self {
        Self {
            state: Mutex::new(HeapAllocatorState {
                slabs: SlabAllocator::new(),
                allocations: 0,
                requested_bytes: 0,
                allocated_bytes: 0,
                mapped_frames: 0,
            }),
        }
    }

    pub fn allocate(&self, layout: Layout) -> Result<HeapAllocation, MemoryError> {
        if layout.size() == 0 {
            return Ok(HeapAllocation {
                layout,
                storage: HeapStorage::Empty,
            });
        }
        if !manager().direct_map.is_active() {
            return Err(MemoryError::NotInitialized);
        }
        let mut state = self.state.lock();
        let storage = match state.allocate(layout) {
            Err(MemoryError::NoMemory) if state.slabs.reclaim() != 0 => state.allocate(layout)?,
            result => result?,
        };
        let allocation = HeapAllocation { layout, storage };
        state.allocations += 1;
        state.requested_bytes += layout.size();
        state.allocated_bytes += allocation.capacity();
        Ok(allocation)
    }

    pub fn stats(&self) -> HeapStats {
        let state = self.state.lock();
        HeapStats {
            allocations: state.allocations,
            requested_bytes: state.requested_bytes,
            allocated_bytes: state.allocated_bytes,
            backing_frames: state.slabs.frames + state.mapped_frames,
            metadata_frames: state.slabs.metadata_frames,
            cached_frames: state
                .slabs
                .empty
                .iter()
                .filter(|&&record| record != NIL)
                .count(),
        }
    }

    pub fn reclaim(&self) -> usize {
        self.state.lock().slabs.reclaim()
    }

    fn release(&self, allocation: &HeapAllocation) {
        if matches!(allocation.storage, HeapStorage::Empty) {
            return;
        }
        let mut state = self.state.lock();
        match &allocation.storage {
            HeapStorage::Slab(slab) => state.slabs.release(slab),
            HeapStorage::Mapped(mapping) => {
                manager()
                    .kernel_address_space()
                    .unmap_owned(mapping)
                    .expect("failed to unmap heap allocation");
                state.mapped_frames -= allocation.capacity() / PAGE_SIZE;
            }
            HeapStorage::Empty => unreachable!(),
        }
        state.allocations -= 1;
        state.requested_bytes -= allocation.layout.size();
        state.allocated_bytes -= allocation.capacity();
    }
}

impl HeapAllocatorState {
    fn allocate(&mut self, layout: Layout) -> Result<HeapStorage, MemoryError> {
        if let Some(class) = size_class(layout) {
            return self.slabs.allocate(class).map(HeapStorage::Slab);
        }
        let length = layout
            .size()
            .checked_add(PAGE_SIZE - 1)
            .map(|size| size & !(PAGE_SIZE - 1))
            .ok_or(MemoryError::NoMemory)?;
        let frames = length / PAGE_SIZE;
        let backing = manager().allocate(FrameOwner::Kernel, FramePurpose::Data, frames as u64)?;
        let mapping = manager().kernel_address_space().map_owned(
            &backing,
            0..frames as u64,
            MappingOptions {
                alignment: layout.align().max(PAGE_SIZE),
                ..MappingOptions::new(PagePermissions::ReadWrite)
            },
        )?;
        self.mapped_frames += frames;
        Ok(HeapStorage::Mapped(mapping))
    }
}

impl HeapAllocation {
    pub fn address(&self) -> usize {
        match &self.storage {
            HeapStorage::Empty => self.layout.align(),
            HeapStorage::Slab(slab) => slab.address,
            HeapStorage::Mapped(mapping) => mapping.range.start,
        }
    }

    pub fn layout(&self) -> Layout {
        self.layout
    }

    pub fn capacity(&self) -> usize {
        match &self.storage {
            HeapStorage::Empty => 0,
            HeapStorage::Slab(_) => class_size(size_class(self.layout).unwrap()),
            HeapStorage::Mapped(mapping) => mapping.range.end - mapping.range.start,
        }
    }
}

impl Drop for HeapAllocation {
    fn drop(&mut self) {
        manager().heap_allocator.release(self);
    }
}

impl SlabAllocator {
    const fn new() -> Self {
        Self {
            partial: [NIL; CLASSES],
            empty: [NIL; CLASSES],
            free_records: NIL,
            frames: 0,
            metadata_frames: 0,
        }
    }

    fn allocate(&mut self, class: usize) -> Result<SlabAllocation, MemoryError> {
        let (record, access) = if self.partial[class] == NIL {
            self.create(class)?
        } else {
            let record = self.partial[class];
            let slab = Slab::read(record);
            let access = manager()
                .reference(slab.backing, ReferenceKind::Temporary)?
                .access(slab.frame, FrameAccess::Write)?;
            (record, access)
        };
        let mut slab = Slab::read(record);
        let word = slab
            .bitmap
            .iter()
            .position(|word| *word != u64::MAX)
            .unwrap();
        let bit = slab.bitmap[word].trailing_ones() as usize;
        let slot = word * u64::BITS as usize + bit;
        slab.bitmap[word] |= 1 << bit;
        slab.used += 1;
        let address = slab.address + slot * class_size(class);
        slab.write(record);
        if self.empty[class] == record {
            self.empty[class] = NIL;
        }
        if slab.used == PAGE_SIZE / class_size(class) {
            self.unlink_partial(record);
        }
        Ok(SlabAllocation {
            address,
            record,
            _access: access,
        })
    }

    fn create(&mut self, class: usize) -> Result<(u64, FrameAccessReference), MemoryError> {
        let backing = manager().allocate(FrameOwner::Kernel, FramePurpose::Data, 1)?;
        let frame = backing.frames().next().unwrap();
        let access = backing.access(frame, ReferenceKind::Temporary, FrameAccess::Write)?;
        let address = access
            .virtual_address()
            .ok_or(MemoryError::NotInitialized)?;
        let record = self.allocate_record()?;
        let slots = PAGE_SIZE / class_size(class);
        let bitmap = core::array::from_fn(|word| {
            let free = slots.saturating_sub(word * u64::BITS as usize).min(64);
            u64::MAX.checked_shl(free as u32).unwrap_or(0)
        });
        Slab {
            backing: backing.head,
            frame,
            address,
            previous: NIL,
            next: NIL,
            class,
            used: 0,
            bitmap,
        }
        .write(record);
        // Page frame ownership transfers to the metadata record and is reclaimed when the slab is released.
        core::mem::forget(backing);
        self.frames += 1;
        self.link_partial(record);
        Ok((record, access))
    }

    fn release(&mut self, allocation: &SlabAllocation) {
        let record = allocation.record;
        let mut slab = Slab::read(record);
        let size = class_size(slab.class);
        let slot = (allocation.address - slab.address) / size;
        let was_full = slab.used == PAGE_SIZE / size;
        slab.bitmap[slot / 64] &= !(1 << (slot % 64));
        slab.used -= 1;
        slab.write(record);
        if was_full {
            self.link_partial(record);
        }
        if slab.used == 0 {
            if self.empty[slab.class] == NIL {
                self.empty[slab.class] = record;
            } else {
                self.release_slab(record);
            }
        }
    }

    fn reclaim(&mut self) -> usize {
        let before = self.frames + self.metadata_frames;
        for class in 0..CLASSES {
            if self.empty[class] != NIL {
                self.release_slab(self.empty[class]);
            }
        }
        before - self.frames - self.metadata_frames
    }

    fn release_slab(&mut self, record: u64) {
        let slab = Slab::read(record);
        self.unlink_partial(record);
        if self.empty[slab.class] == record {
            self.empty[slab.class] = NIL;
        }
        drop(FrameAllocation { head: slab.backing });
        self.frames -= 1;
        self.release_record(record);
    }

    fn link_partial(&mut self, record: u64) {
        let mut slab = Slab::read(record);
        slab.previous = NIL;
        slab.next = self.partial[slab.class];
        if slab.next != NIL {
            let mut next = Slab::read(slab.next);
            next.previous = record;
            next.write(slab.next);
        }
        self.partial[slab.class] = record;
        slab.write(record);
    }

    fn unlink_partial(&mut self, record: u64) {
        let mut slab = Slab::read(record);
        if slab.previous == NIL {
            self.partial[slab.class] = slab.next;
        } else {
            let mut previous = Slab::read(slab.previous);
            previous.next = slab.next;
            previous.write(slab.previous);
        }
        if slab.next != NIL {
            let mut next = Slab::read(slab.next);
            next.previous = slab.previous;
            next.write(slab.next);
        }
        slab.previous = NIL;
        slab.next = NIL;
        slab.write(record);
    }

    fn allocate_record(&mut self) -> Result<u64, MemoryError> {
        if self.free_records == NIL {
            let backing = manager().allocate(FrameOwner::Kernel, FramePurpose::Data, 1)?;
            let frame = backing.frames().next().unwrap().address();
            SlabMetadata {
                backing: backing.head,
                previous: NIL,
                next: NIL,
                occupied: 0,
            }
            .write(frame);
            core::mem::forget(backing);
            self.free_records = frame;
            self.metadata_frames += 1;
        }
        let page = self.free_records;
        let mut header = SlabMetadata::read(page);
        let slot = ((!header.occupied) & FULL).trailing_zeros() as usize;
        header.occupied |= 1 << slot;
        header.write(page);
        if header.occupied == FULL {
            self.unlink_free_metadata(page);
        }
        Ok(page + ((HEADER_WORDS + slot * RECORD_WORDS) * size_of::<u64>()) as u64)
    }

    fn release_record(&mut self, record: u64) {
        let page = record & !(PAGE_SIZE as u64 - 1);
        let slot = (record as usize % PAGE_SIZE / size_of::<u64>() - HEADER_WORDS) / RECORD_WORDS;
        let mut header = SlabMetadata::read(page);
        let was_full = header.occupied == FULL;
        header.occupied &= !(1 << slot);
        header.write(page);
        if was_full {
            header.previous = NIL;
            header.next = self.free_records;
            if self.free_records != NIL {
                let mut next = SlabMetadata::read(self.free_records);
                next.previous = page;
                next.write(self.free_records);
            }
            self.free_records = page;
            header.write(page);
        }
        if header.occupied == 0 {
            self.unlink_free_metadata(page);
            drop(FrameAllocation {
                head: header.backing,
            });
            self.metadata_frames -= 1;
        }
    }

    fn unlink_free_metadata(&mut self, page: u64) {
        let mut header = SlabMetadata::read(page);
        if header.previous == NIL {
            self.free_records = header.next;
        } else {
            let mut previous = SlabMetadata::read(header.previous);
            previous.next = header.next;
            previous.write(header.previous);
        }
        if header.next != NIL {
            let mut next = SlabMetadata::read(header.next);
            next.previous = header.previous;
            next.write(header.next);
        }
        header.previous = NIL;
        header.next = NIL;
        header.write(page);
    }
}

impl Slab {
    fn read(address: u64) -> Self {
        let words = read::<RECORD_WORDS>(address);
        Self {
            backing: words[0],
            frame: Frame::new(words[1]).unwrap(),
            address: words[2] as usize,
            previous: words[3],
            next: words[4],
            class: words[5] as usize,
            used: words[6] as usize,
            bitmap: words[7..].try_into().unwrap(),
        }
    }

    fn write(&self, address: u64) {
        let mut words = [0; RECORD_WORDS];
        words[..7].copy_from_slice(&[
            self.backing,
            self.frame.address(),
            self.address as u64,
            self.previous,
            self.next,
            self.class as u64,
            self.used as u64,
        ]);
        words[7..].copy_from_slice(&self.bitmap);
        write(address, &words);
    }
}

impl SlabMetadata {
    fn read(address: u64) -> Self {
        let words = read::<HEADER_WORDS>(address);
        Self {
            backing: words[0],
            previous: words[1],
            next: words[2],
            occupied: words[3],
        }
    }

    fn write(&self, address: u64) {
        write(
            address,
            &[self.backing, self.previous, self.next, self.occupied],
        );
    }
}

fn read<const N: usize>(address: u64) -> [u64; N] {
    let mut words = [0; N];
    hardware::mmu()
        .read_frame(
            address & !(PAGE_SIZE as u64 - 1),
            address as usize % PAGE_SIZE / size_of::<u64>(),
            &mut words,
        )
        .expect("failed to read slab metadata");
    words
}

fn write(address: u64, words: &[u64]) {
    hardware::mmu()
        .write_frame(
            address & !(PAGE_SIZE as u64 - 1),
            address as usize % PAGE_SIZE / size_of::<u64>(),
            words,
        )
        .expect("failed to write slab metadata");
}

fn size_class(layout: Layout) -> Option<usize> {
    let size = layout.size().max(layout.align()).max(MIN_SIZE);
    let class = size.checked_next_power_of_two()?.trailing_zeros() as usize
        - MIN_SIZE.trailing_zeros() as usize;
    (class < CLASSES).then_some(class)
}

const fn class_size(class: usize) -> usize {
    MIN_SIZE << class
}
