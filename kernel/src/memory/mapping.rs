// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use core::ops::BitOr;
use core::sync::atomic::Ordering;

use super::{Frame, FrameAllocation, FrameOwner, FramePurpose, MemoryError, VirtRange, manager};
use crate::hardware::{
    self,
    mmu::{MemoryType, PAGE_SIZE, PageAttributes, PageMapping, PagePermissions},
};

const NIL: u64 = u64::MAX;
const HEADER_WORDS: usize = 4;
const TREE_WORDS: usize = 9;
const RECORD_WORDS: usize = 9;
const NODE_WORDS: usize = TREE_WORDS + RECORD_WORDS;
const SLOTS: usize = (PAGE_SIZE / 8 - HEADER_WORDS) / NODE_WORDS;
const FULL: u64 = (1 << SLOTS) - 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MappingOptions {
    pub permissions: PagePermissions,
    pub maximum: MappingRights,
    pub alignment: usize,
}

impl MappingOptions {
    pub fn new(permissions: PagePermissions) -> Self {
        Self {
            permissions,
            maximum: permissions.into(),
            alignment: PAGE_SIZE,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MappingInfo {
    pub range: VirtRange,
    pub mapping: Option<PageMapping>,
    pub maximum: MappingRights,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MappingRights(pub(super) u8);

impl MappingRights {
    pub const READ: Self = Self(1);
    pub const WRITE: Self = Self(2);
    pub const EXECUTE: Self = Self(4);

    pub(super) fn allows(self, permissions: PagePermissions) -> bool {
        let requested = Self::from(permissions).0;
        self.0 & requested == requested
    }
}

impl BitOr for MappingRights {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl From<PagePermissions> for MappingRights {
    fn from(permissions: PagePermissions) -> Self {
        Self(match permissions {
            PagePermissions::NoAccess => 0,
            PagePermissions::ReadOnly => 1,
            PagePermissions::ReadWrite => 3,
            PagePermissions::ReadExecute => 5,
            PagePermissions::ReadWriteExecute => 7,
            PagePermissions::ExecuteOnly => 4,
        })
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum MappingKind {
    Reserved,
    Preparing,
    Mapped,
    Huge,
    Permanent { physical_start: u64 },
    DirectMap,
    Owned,
    Device { physical_start: u64 },
    Guard,
}

pub(super) struct OwnedMapping {
    pub range: VirtRange,
    pub generation: u64,
}

#[derive(Clone, Copy, Default)]
// Each record reserves rollback space so that unmapping and permission downgrades require no allocation.
pub(super) struct MappingJournal {
    pub tables: [Option<Frame>; 4],
    pub mapping: Option<PageMapping>,
    pub detached: u64,
    pub written: bool,
    pub referenced: bool,
    pub direct_was_writable: bool,
    pub reserved: u64,
}

impl MappingJournal {
    pub fn frame(self) -> Frame {
        Frame::new(
            self.mapping
                .expect("missing staged mapping")
                .physical_address,
        )
        .expect("unaligned mapping frame")
    }

    pub fn permissions(self) -> PagePermissions {
        self.mapping
            .expect("missing staged mapping")
            .attributes
            .permissions
    }

    /// Page table level of the staged leaf; zero for base pages.
    pub fn level(self) -> usize {
        self.tables
            .iter()
            .position(Option::is_some)
            .expect("missing staged page table")
    }
}

#[derive(Clone, Copy)]
pub(super) struct MappingRecord {
    pub range: VirtRange,
    pub maximum: MappingRights,
    pub generation: u64,
    pub kind: MappingKind,
    pub journal: MappingJournal,
}

impl MappingRecord {
    /// Physical frame backing `address`, accounting for large leaf mappings.
    pub fn frame_at(self, address: usize) -> Frame {
        let offset = u64::try_from(address - self.range.start).expect("address outside mapping");
        Frame::new(
            self.journal
                .mapping
                .expect("missing staged mapping")
                .physical_address
                + offset,
        )
        .expect("unaligned mapping frame")
    }
}

pub(super) struct MappingIndex {
    root: u64,
    free: u64,
    backing: Option<FrameAllocation>,
}

#[derive(Clone, Copy)]
struct MappingNode {
    range: VirtRange,
    parent: u64,
    left: u64,
    right: u64,
    height: u64,
    first: usize,
    last: usize,
    gap: usize,
}

impl MappingIndex {
    pub fn new() -> Self {
        Self {
            root: NIL,
            free: NIL,
            backing: None,
        }
    }

    pub fn len(&self) -> usize {
        fn count(address: u64) -> usize {
            if address == NIL {
                return 0;
            }
            let node = MappingNode::read(address);
            1 + count(node.left) + count(node.right)
        }
        count(self.root)
    }

    pub fn any(&self, predicate: impl Fn(&MappingRecord) -> bool) -> bool {
        fn visit(address: u64, predicate: &impl Fn(&MappingRecord) -> bool) -> bool {
            if address == NIL {
                return false;
            }
            let node = MappingNode::read(address);
            predicate(&MappingRecord::read(address, node.range))
                || visit(node.left, predicate)
                || visit(node.right, predicate)
        }
        visit(self.root, &predicate)
    }

    pub fn get(&self, address: usize) -> Option<MappingRecord> {
        self.find(address)
            .map(|(address, range)| MappingRecord::read(address, range))
    }

    pub fn update<R>(
        &mut self,
        address: usize,
        update: impl FnOnce(&mut MappingRecord) -> R,
    ) -> Option<R> {
        let (location, range) = self.find(address)?;
        let mut record = MappingRecord::read(location, range);
        let result = update(&mut record);
        assert_eq!(range, record.range, "mapping extent changed");
        // Write back before returning the callback result so rollback sees any completed changes.
        record.write(location);
        Some(result)
    }

    pub fn query(&self, address: usize, next: bool) -> Option<(MappingInfo, bool)> {
        let mut current = self.root;
        let mut candidate = None;
        while current != NIL {
            let (range, left, right) = MappingNode::search_fields(current);
            if address < range.start {
                if next {
                    candidate = Some((current, range));
                }
                current = left;
            } else if address >= range.end {
                current = right;
            } else {
                candidate = Some((current, range));
                break;
            }
        }
        let (location, range) = candidate?;
        let mut words = [0; 2];
        read(
            location + (TREE_WORDS * size_of::<u64>()) as u64,
            &mut words,
        );
        let mapped = match words[1] >> 8 {
            0 | 7 => false,
            1..=6 | 8 => true,
            _ => panic!("invalid mapping kind"),
        };
        Some((
            MappingInfo {
                range,
                mapping: None,
                maximum: MappingRights(words[1] as u8),
                generation: words[0],
            },
            mapped,
        ))
    }

    fn find(&self, address: usize) -> Option<(u64, VirtRange)> {
        let mut current = self.root;
        while current != NIL {
            let (range, left, right) = MappingNode::search_fields(current);
            if address < range.start {
                current = left;
            } else if address >= range.end {
                current = right;
            } else {
                return Some((current, range));
            }
        }
        None
    }

    pub fn set(&mut self, record: MappingRecord) {
        let (address, range) = self
            .find(record.range.start)
            .expect("missing mapping record");
        assert_eq!(range, record.range, "mapping extent changed");
        record.write(address);
    }

    pub fn overlaps(&self, range: VirtRange) -> bool {
        let mut current = self.root;
        while current != NIL {
            let (current_range, left, right) = MappingNode::search_fields(current);
            if current_range.overlaps(range) {
                return true;
            }
            current = if range.end <= current_range.start {
                left
            } else {
                right
            };
        }
        false
    }

    pub fn insert(&mut self, record: MappingRecord) -> Result<(), MemoryError> {
        let mut parent = NIL;
        let mut current = self.root;
        while current != NIL {
            parent = current;
            let (range, left, right) = MappingNode::search_fields(current);
            if range.overlaps(record.range) {
                return Err(MemoryError::AlreadyMapped);
            }
            current = if record.range.start < range.start {
                left
            } else {
                right
            };
        }
        let address = self.allocate_node()?;
        MappingNode {
            range: record.range,
            parent,
            left: NIL,
            right: NIL,
            height: 1,
            first: record.range.start,
            last: record.range.end,
            gap: 0,
        }
        .write(address);
        record.write(address);
        if parent == NIL {
            self.root = address;
        } else {
            let (range, _, _) = MappingNode::search_fields(parent);
            let field = if record.range.start < range.start {
                2
            } else {
                3
            };
            write(parent + field * size_of::<u64>() as u64, &[address]);
            self.rebalance(parent);
        }
        Ok(())
    }

    pub fn remove(&mut self, start: usize) -> MappingRecord {
        let (address, _) = self.find(start).expect("missing mapping record");
        let node = MappingNode::read(address);
        assert_eq!(node.range.start, start, "partial reservation removal");
        let record = MappingRecord::read(address, node.range);
        let rebalance = if node.left != NIL && node.right != NIL {
            let mut successor = node.right;
            let mut replacement = MappingNode::read(successor);
            while replacement.left != NIL {
                successor = replacement.left;
                replacement = MappingNode::read(successor);
            }
            // Relink the successor so surviving records retain their backing pages.
            let rebalance = if replacement.parent == address {
                successor
            } else {
                let parent = replacement.parent;
                self.replace_child(parent, successor, replacement.right);
                if replacement.right != NIL {
                    write(replacement.right + 4 * size_of::<u64>() as u64, &[parent]);
                }
                replacement.right = node.right;
                write(node.right + 4 * size_of::<u64>() as u64, &[successor]);
                parent
            };
            replacement.left = node.left;
            replacement.parent = node.parent;
            replacement.write(successor);
            write(node.left + 4 * size_of::<u64>() as u64, &[successor]);
            self.replace_child(node.parent, address, successor);
            rebalance
        } else {
            let child = if node.left != NIL {
                node.left
            } else {
                node.right
            };
            self.replace_child(node.parent, address, child);
            if child != NIL {
                write(child + 4 * size_of::<u64>() as u64, &[node.parent]);
            }
            node.parent
        };
        self.rebalance(rebalance);
        if self.root == NIL {
            self.free = NIL;
            self.backing = None;
        } else {
            self.free_node(address);
        }
        record
    }

    pub fn gap(&self, bounds: VirtRange, length: usize, alignment: usize) -> Option<VirtRange> {
        let mut candidate = align(bounds.start, alignment)?;
        Self::search_gap(self.root, &mut candidate, length, alignment)?;
        candidate
            .checked_add(length)
            .filter(|end| *end <= bounds.end)
            .map(|end| VirtRange {
                start: candidate,
                end,
            })
    }

    fn search_gap(
        address: u64,
        candidate: &mut usize,
        length: usize,
        alignment: usize,
    ) -> Option<bool> {
        if address == NIL {
            return Some(false);
        }
        let node = MappingNode::read(address);
        if node.last <= *candidate {
            return Some(false);
        }
        if candidate.checked_add(length)? <= node.first {
            return Some(true);
        }
        if node.gap < length {
            *candidate = align((*candidate).max(node.last), alignment)?;
            return Some(false);
        }
        if Self::search_gap(node.left, candidate, length, alignment)? {
            return Some(true);
        }
        if candidate.checked_add(length)? <= node.range.start {
            return Some(true);
        }
        *candidate = align((*candidate).max(node.range.end), alignment)?;
        Self::search_gap(node.right, candidate, length, alignment)
    }

    fn replace_child(&mut self, parent: u64, old: u64, new: u64) {
        if parent == NIL {
            self.root = new;
            return;
        }
        let (_, left, right) = MappingNode::search_fields(parent);
        let field = if left == old {
            2
        } else {
            assert_eq!(right, old);
            3
        };
        write(parent + field * size_of::<u64>() as u64, &[new]);
    }

    fn refresh(address: u64) -> (MappingNode, i64) {
        let mut node = MappingNode::read(address);
        let mut left_height = 0;
        let mut right_height = 0;
        node.first = node.range.start;
        node.last = node.range.end;
        node.gap = 0;
        if node.left != NIL {
            let [height, first, last, gap] = MappingNode::summary(node.left);
            left_height = height;
            node.first = first as usize;
            node.gap = (gap as usize).max(node.range.start - last as usize);
        }
        if node.right != NIL {
            let [height, first, last, gap] = MappingNode::summary(node.right);
            right_height = height;
            node.last = last as usize;
            node.gap = node
                .gap
                .max(gap as usize)
                .max(first as usize - node.range.end);
        }
        node.height = left_height.max(right_height) + 1;
        write(
            address + 5 * size_of::<u64>() as u64,
            &[
                node.height,
                node.first as u64,
                node.last as u64,
                node.gap as u64,
            ],
        );
        (node, left_height as i64 - right_height as i64)
    }

    fn balance(address: u64) -> i64 {
        let node = MappingNode::read(address);
        let height = |child| {
            if child == NIL {
                0
            } else {
                let mut height = [0];
                read(child + 5 * size_of::<u64>() as u64, &mut height);
                height[0] as i64
            }
        };
        height(node.left) - height(node.right)
    }

    fn rotate(&mut self, address: u64, left: bool) -> u64 {
        let mut upper = MappingNode::read(address);
        let child = if left { upper.right } else { upper.left };
        let mut lower = MappingNode::read(child);
        let middle = if left { lower.left } else { lower.right };
        if left {
            upper.right = middle;
            lower.left = address;
        } else {
            upper.left = middle;
            lower.right = address;
        }
        lower.parent = upper.parent;
        upper.parent = child;
        upper.write(address);
        lower.write(child);
        if middle != NIL {
            write(middle + 4 * size_of::<u64>() as u64, &[address]);
        }
        self.replace_child(lower.parent, address, child);
        Self::refresh(address);
        Self::refresh(child);
        child
    }

    // REFERENCE: https://en.wikipedia.org/wiki/AVL_tree
    fn rebalance(&mut self, mut address: u64) {
        while address != NIL {
            let (node, balance) = Self::refresh(address);
            if balance > 1 {
                let child = node.left;
                if Self::balance(child) < 0 {
                    self.rotate(child, true);
                }
                self.rotate(address, false);
            } else if balance < -1 {
                let child = node.right;
                if Self::balance(child) > 0 {
                    self.rotate(child, false);
                }
                self.rotate(address, true);
            }
            address = node.parent;
        }
    }

    fn allocate_node(&mut self) -> Result<u64, MemoryError> {
        if self.free == NIL {
            let allocation = manager().allocate(FrameOwner::Kernel, FramePurpose::Data, 1)?;
            let frame = allocation.frames().next().unwrap();
            {
                let _allocator = manager().allocator().lock();
                // Reclaimed boot pages and former executable pages may still be read-only in the direct map.
                manager().direct_map.set_writable(frame, true)?;
            }
            let physical = frame.address();
            let frame = manager()
                .direct_map
                .active_address(frame)
                .map_or(physical, |address| address as u64);
            write(frame, &[0, NIL, NIL, physical]);
            if let Some(backing) = &mut self.backing {
                backing.merge(allocation);
            } else {
                self.backing = Some(allocation);
            }
            self.free = frame;
        }
        let page = self.free;
        let mut header = [0; HEADER_WORDS];
        read(page, &mut header);
        let slot = ((!header[0]) & FULL).trailing_zeros() as usize;
        header[0] |= 1 << slot;
        write(page, &header);
        if header[0] == FULL {
            self.unlink(page);
        }
        Ok(page + ((HEADER_WORDS + slot * NODE_WORDS) * 8) as u64)
    }

    fn free_node(&mut self, address: u64) {
        let page = address & !(PAGE_SIZE as u64 - 1);
        let slot = (address as usize % PAGE_SIZE / 8 - HEADER_WORDS) / NODE_WORDS;
        let mut header = [0; HEADER_WORDS];
        read(page, &mut header);
        let was_full = header[0] == FULL;
        header[0] &= !(1 << slot);
        if was_full {
            header[1] = NIL;
            header[2] = self.free;
            if self.free != NIL {
                let mut next = [0; HEADER_WORDS];
                read(self.free, &mut next);
                next[1] = page;
                write(self.free, &next);
            }
            self.free = page;
        }
        write(page, &header);
        let backing = self.backing.as_ref().unwrap();
        if header[0] == 0 && backing.info().frames > 1 {
            self.unlink(page);
            drop(
                self.backing
                    .as_mut()
                    .unwrap()
                    .detach(Frame::new(header[3]).unwrap()),
            );
        }
    }

    fn unlink(&mut self, page: u64) {
        let mut header = [0; HEADER_WORDS];
        read(page, &mut header);
        if header[1] == NIL {
            self.free = header[2];
        } else {
            let mut previous = [0; HEADER_WORDS];
            read(header[1], &mut previous);
            previous[2] = header[2];
            write(header[1], &previous);
        }
        if header[2] != NIL {
            let mut next = [0; HEADER_WORDS];
            read(header[2], &mut next);
            next[1] = header[1];
            write(header[2], &next);
        }
    }
}

impl MappingNode {
    #[inline(always)]
    fn summary(address: u64) -> [u64; 4] {
        let mut words = [0; 4];
        read(address + 5 * size_of::<u64>() as u64, &mut words);
        words
    }

    #[inline(always)]
    fn search_fields(address: u64) -> (VirtRange, u64, u64) {
        let mut words = [0; 4];
        read(address, &mut words);
        (
            VirtRange {
                start: words[0] as usize,
                end: words[1] as usize,
            },
            words[2],
            words[3],
        )
    }

    fn read(address: u64) -> Self {
        let mut words = [0; TREE_WORDS];
        read(address, &mut words);
        Self {
            range: VirtRange {
                start: words[0] as usize,
                end: words[1] as usize,
            },
            left: words[2],
            right: words[3],
            parent: words[4],
            height: words[5],
            first: words[6] as usize,
            last: words[7] as usize,
            gap: words[8] as usize,
        }
    }

    fn write(self, address: u64) {
        write(
            address,
            &[
                self.range.start as u64,
                self.range.end as u64,
                self.left,
                self.right,
                self.parent,
                self.height,
                self.first as u64,
                self.last as u64,
                self.gap as u64,
            ],
        );
    }
}

impl MappingRecord {
    fn read(address: u64, range: VirtRange) -> Self {
        let mut words = [0; RECORD_WORDS];
        read(address + (TREE_WORDS * size_of::<u64>()) as u64, &mut words);
        let mapping = (words[8] & 64 != 0).then(|| PageMapping {
            physical_address: words[6],
            attributes: PageAttributes {
                permissions: match words[7] & 19 {
                    0 => PagePermissions::ReadOnly,
                    1 => PagePermissions::ReadWrite,
                    2 => PagePermissions::ReadExecute,
                    16 => PagePermissions::NoAccess,
                    17 => PagePermissions::ReadWriteExecute,
                    _ => PagePermissions::ExecuteOnly,
                },
                memory_type: if words[7] & 4 != 0 {
                    MemoryType::Device
                } else {
                    MemoryType::Normal
                },
                user_accessible: words[7] & 8 != 0,
            },
        });
        Self {
            range,
            generation: words[0],
            maximum: MappingRights(words[1] as u8),
            kind: match words[1] >> 8 {
                0 => MappingKind::Reserved,
                1 => MappingKind::Preparing,
                2 => MappingKind::Mapped,
                3 => MappingKind::Permanent {
                    physical_start: words[6],
                },
                4 => MappingKind::DirectMap,
                5 => MappingKind::Owned,
                6 => MappingKind::Device {
                    physical_start: words[6],
                },
                7 => MappingKind::Guard,
                8 => MappingKind::Huge,
                _ => panic!("invalid mapping kind"),
            },
            journal: MappingJournal {
                tables: core::array::from_fn(|i| {
                    (words[2 + i] != NIL).then(|| Frame::new(words[2 + i]).unwrap())
                }),
                mapping,
                reserved: words[8] >> 8,
                detached: words[8] & 15,
                written: words[8] & 16 != 0,
                referenced: words[8] & 32 != 0,
                direct_was_writable: words[8] & 128 != 0,
            },
        }
    }

    fn write(self, address: u64) {
        let record = self;
        let journal = record.journal;
        let (mut physical, attributes) = journal.mapping.map_or((0, 0), |mapping| {
            let attributes = mapping.attributes;
            (
                mapping.physical_address,
                match attributes.permissions {
                    PagePermissions::NoAccess => 16,
                    PagePermissions::ReadWriteExecute => 17,
                    PagePermissions::ReadOnly => 0,
                    PagePermissions::ReadWrite => 1,
                    PagePermissions::ReadExecute => 2,
                    PagePermissions::ExecuteOnly => 3,
                } | u64::from(attributes.memory_type == MemoryType::Device) << 2
                    | u64::from(attributes.user_accessible) << 3,
            )
        });
        let kind = match record.kind {
            MappingKind::Reserved => 0,
            MappingKind::Preparing => 1,
            MappingKind::Mapped => 2,
            MappingKind::Permanent { physical_start } => {
                physical = physical_start;
                3
            }
            MappingKind::DirectMap => 4,
            MappingKind::Owned => 5,
            MappingKind::Device { physical_start } => {
                physical = physical_start;
                6
            }
            MappingKind::Guard => 7,
            MappingKind::Huge => 8,
        };
        write(
            address + (TREE_WORDS * size_of::<u64>()) as u64,
            &[
                record.generation,
                record.maximum.0 as u64 | kind << 8,
                journal.tables[0].map_or(NIL, Frame::address),
                journal.tables[1].map_or(NIL, Frame::address),
                journal.tables[2].map_or(NIL, Frame::address),
                journal.tables[3].map_or(NIL, Frame::address),
                physical,
                attributes,
                journal.detached
                    | u64::from(journal.written) << 4
                    | u64::from(journal.referenced) << 5
                    | u64::from(journal.mapping.is_some()) << 6
                    | u64::from(journal.direct_was_writable) << 7
                    | journal.reserved << 8,
            ],
        );
    }
}

fn align(address: usize, alignment: usize) -> Option<usize> {
    address
        .checked_add(alignment - 1)
        .map(|value| value & !(alignment - 1))
}

#[inline(always)]
fn read(address: u64, words: &mut [u64]) {
    let frame = Frame::new(address & !(PAGE_SIZE as u64 - 1)).unwrap();
    let offset = address as usize % PAGE_SIZE / 8;
    let direct_map = &super::MEMORY_MANAGER.direct_map;
    let mapped = if address > isize::MAX as u64 {
        Some(frame.address() as usize)
    } else {
        direct_map.active_address(frame)
    };
    let result = if let Some(address) = mapped {
        // SAFETY: The index owns its backing pages exclusively and keeps them writable. The address-space state lock serializes access.
        let mapped = hardware::mmu()
            .mapped_atomic_words(address)
            .expect("invalid metadata mapping");
        let source = &mapped[offset..offset + words.len()];
        for (word, source) in words.iter_mut().zip(source) {
            *word = source.load(Ordering::Relaxed);
        }
        Ok(())
    } else {
        assert!(
            !direct_map.is_active(),
            "mapping metadata is not directly mapped"
        );
        hardware::mmu().read_frame(frame.address(), offset, words)
    };
    result.expect("failed to read mapping metadata");
}

#[inline(always)]
fn write(address: u64, words: &[u64]) {
    let frame = Frame::new(address & !(PAGE_SIZE as u64 - 1)).unwrap();
    let offset = address as usize % PAGE_SIZE / 8;
    let direct_map = &super::MEMORY_MANAGER.direct_map;
    let mapped = if address > isize::MAX as u64 {
        Some(frame.address() as usize)
    } else {
        direct_map.active_address(frame)
    };
    let result = if let Some(address) = mapped {
        // SAFETY: The index owns its backing pages exclusively and keeps them writable. The address-space state lock serializes access.
        let mapped = hardware::mmu()
            .mapped_atomic_words(address)
            .expect("invalid metadata mapping");
        for (target, word) in mapped[offset..offset + words.len()].iter().zip(words) {
            target.store(*word, Ordering::Relaxed);
        }
        Ok(())
    } else {
        assert!(
            !direct_map.is_active(),
            "mapping metadata is not directly mapped"
        );
        hardware::mmu().write_frame(frame.address(), offset, words)
    };
    result.expect("failed to write mapping metadata");
}
