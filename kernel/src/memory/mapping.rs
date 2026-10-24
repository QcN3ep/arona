// SPDX-License-Identifier: MPL-2.0

use core::ops::BitOr;

use super::{Frame, FrameAllocation, FrameOwner, FramePurpose, MemoryError, VirtRange, manager};
use crate::hardware::{
    self,
    mmu::{MemoryType, PAGE_SIZE, PageAttributes, PageMapping, PagePermissions},
};

const NIL: u64 = u64::MAX;
const HEADER_WORDS: usize = 3;
const NODE_WORDS: usize = 18;
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
            PagePermissions::ReadOnly => 1,
            PagePermissions::ReadWrite => 3,
            PagePermissions::ReadExecute => 5,
            PagePermissions::ExecuteOnly => 4,
        })
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum MappingKind {
    Reserved,
    Preparing,
    Mapped,
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
// 每条记录预留回滚空间，使解除映射和权限降低无需分配。
pub(super) struct MappingJournal {
    pub tables: [Option<Frame>; 4],
    pub mapping: Option<PageMapping>,
    pub detached: u64,
    pub written: bool,
    pub referenced: bool,
    pub direct_was_writable: bool,
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
}

#[derive(Clone, Copy)]
pub(super) struct MappingRecord {
    pub range: VirtRange,
    pub maximum: MappingRights,
    pub generation: u64,
    pub kind: MappingKind,
    pub journal: MappingJournal,
}

pub(super) struct MappingIndex {
    root: u64,
    free: u64,
    backing: Option<FrameAllocation>,
}

#[derive(Clone, Copy)]
struct MappingNode {
    record: MappingRecord,
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
            predicate(&node.record) || visit(node.left, predicate) || visit(node.right, predicate)
        }
        visit(self.root, &predicate)
    }

    pub fn get(&self, address: usize) -> Option<MappingRecord> {
        self.find(address)
            .map(|node| MappingNode::read(node).record)
    }

    fn find(&self, address: usize) -> Option<u64> {
        let mut current = self.root;
        while current != NIL {
            let node = MappingNode::read(current);
            if address < node.record.range.start {
                current = node.left;
            } else if address >= node.record.range.end {
                current = node.right;
            } else {
                return Some(current);
            }
        }
        None
    }

    pub fn set(&mut self, record: MappingRecord) {
        let address = self
            .find(record.range.start)
            .expect("missing mapping record");
        let mut node = MappingNode::read(address);
        assert_eq!(node.record.range, record.range, "mapping extent changed");
        node.record = record;
        node.write(address);
    }

    pub fn overlaps(&self, range: VirtRange) -> bool {
        let mut current = self.root;
        while current != NIL {
            let node = MappingNode::read(current);
            if node.record.range.overlaps(range) {
                return true;
            }
            current = if range.end <= node.record.range.start {
                node.left
            } else {
                node.right
            };
        }
        false
    }

    pub fn insert(&mut self, record: MappingRecord) -> Result<(), MemoryError> {
        let mut parent = NIL;
        let mut current = self.root;
        while current != NIL {
            parent = current;
            let node = MappingNode::read(current);
            if node.record.range.overlaps(record.range) {
                return Err(MemoryError::AlreadyMapped);
            }
            current = if record.range.start < node.record.range.start {
                node.left
            } else {
                node.right
            };
        }
        let address = self.allocate_node()?;
        MappingNode {
            record,
            parent,
            left: NIL,
            right: NIL,
            height: 1,
            first: record.range.start,
            last: record.range.end,
            gap: 0,
        }
        .write(address);
        if parent == NIL {
            self.root = address;
        } else {
            let mut node = MappingNode::read(parent);
            if record.range.start < node.record.range.start {
                node.left = address;
            } else {
                node.right = address;
            }
            node.write(parent);
            self.rebalance(parent);
        }
        Ok(())
    }

    pub fn remove(&mut self, start: usize) -> MappingRecord {
        let mut address = self.find(start).expect("missing mapping record");
        let mut node = MappingNode::read(address);
        assert_eq!(
            node.record.range.start, start,
            "partial reservation removal"
        );
        let record = node.record;
        if node.left != NIL && node.right != NIL {
            let mut successor = node.right;
            while MappingNode::read(successor).left != NIL {
                successor = MappingNode::read(successor).left;
            }
            let replacement = MappingNode::read(successor);
            node.record = replacement.record;
            node.write(address);
            address = successor;
            node = replacement;
        }
        let child = if node.left != NIL {
            node.left
        } else {
            node.right
        };
        self.replace_child(node.parent, address, child);
        if child != NIL {
            let mut replacement = MappingNode::read(child);
            replacement.parent = node.parent;
            replacement.write(child);
        }
        self.rebalance(node.parent);
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
        if candidate.checked_add(length)? <= node.record.range.start {
            return Some(true);
        }
        *candidate = align((*candidate).max(node.record.range.end), alignment)?;
        Self::search_gap(node.right, candidate, length, alignment)
    }

    fn replace_child(&mut self, parent: u64, old: u64, new: u64) {
        if parent == NIL {
            self.root = new;
            return;
        }
        let mut node = MappingNode::read(parent);
        if node.left == old {
            node.left = new;
        } else {
            assert_eq!(node.right, old);
            node.right = new;
        }
        node.write(parent);
    }

    fn refresh(address: u64) {
        let mut node = MappingNode::read(address);
        let mut height = 0;
        node.first = node.record.range.start;
        node.last = node.record.range.end;
        node.gap = 0;
        if node.left != NIL {
            let left = MappingNode::read(node.left);
            height = left.height;
            node.first = left.first;
            node.gap = left.gap.max(node.record.range.start - left.last);
        }
        if node.right != NIL {
            let right = MappingNode::read(node.right);
            height = height.max(right.height);
            node.last = right.last;
            node.gap = node
                .gap
                .max(right.gap)
                .max(right.first - node.record.range.end);
        }
        node.height = height + 1;
        node.write(address);
    }

    fn balance(address: u64) -> i64 {
        let node = MappingNode::read(address);
        let height = |child| {
            if child == NIL {
                0
            } else {
                MappingNode::read(child).height as i64
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
            let mut node = MappingNode::read(middle);
            node.parent = address;
            node.write(middle);
        }
        self.replace_child(lower.parent, address, child);
        Self::refresh(address);
        Self::refresh(child);
        child
    }

    // REFERENCE: https://en.wikipedia.org/wiki/AVL_tree
    fn rebalance(&mut self, mut address: u64) {
        while address != NIL {
            Self::refresh(address);
            let balance = Self::balance(address);
            if balance > 1 {
                let child = MappingNode::read(address).left;
                if Self::balance(child) < 0 {
                    self.rotate(child, true);
                }
                address = self.rotate(address, false);
            } else if balance < -1 {
                let child = MappingNode::read(address).right;
                if Self::balance(child) > 0 {
                    self.rotate(child, false);
                }
                address = self.rotate(address, true);
            }
            address = MappingNode::read(address).parent;
        }
    }

    fn allocate_node(&mut self) -> Result<u64, MemoryError> {
        if self.free == NIL {
            let allocation = manager().allocate(FrameOwner::Kernel, FramePurpose::Data, 1)?;
            let frame = allocation.frames().next().unwrap().address();
            write(frame, &[0, NIL, NIL]);
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
        if header[0] == 0 && page != backing.frames().next().unwrap().address() {
            self.unlink(page);
            drop(
                self.backing
                    .as_mut()
                    .unwrap()
                    .detach(Frame::new(page).unwrap()),
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
    fn read(address: u64) -> Self {
        let mut words = [0; NODE_WORDS];
        read(address, &mut words);
        let mapping = (words[10] & 64 != 0).then(|| PageMapping {
            physical_address: words[8],
            attributes: PageAttributes {
                permissions: match words[9] & 3 {
                    0 => PagePermissions::ReadOnly,
                    1 => PagePermissions::ReadWrite,
                    2 => PagePermissions::ReadExecute,
                    _ => PagePermissions::ExecuteOnly,
                },
                memory_type: if words[9] & 4 != 0 {
                    MemoryType::Device
                } else {
                    MemoryType::Normal
                },
                user_accessible: words[9] & 8 != 0,
            },
        });
        Self {
            record: MappingRecord {
                range: VirtRange {
                    start: words[0] as usize,
                    end: words[1] as usize,
                },
                generation: words[2],
                maximum: MappingRights(words[3] as u8),
                kind: match words[3] >> 8 {
                    0 => MappingKind::Reserved,
                    1 => MappingKind::Preparing,
                    2 => MappingKind::Mapped,
                    3 => MappingKind::Permanent {
                        physical_start: words[8],
                    },
                    4 => MappingKind::DirectMap,
                    5 => MappingKind::Owned,
                    6 => MappingKind::Device {
                        physical_start: words[8],
                    },
                    7 => MappingKind::Guard,
                    _ => panic!("invalid mapping kind"),
                },
                journal: MappingJournal {
                    tables: core::array::from_fn(|i| {
                        (words[4 + i] != NIL).then(|| Frame::new(words[4 + i]).unwrap())
                    }),
                    mapping,
                    detached: words[10] & 15,
                    written: words[10] & 16 != 0,
                    referenced: words[10] & 32 != 0,
                    direct_was_writable: words[10] & 128 != 0,
                },
            },
            parent: words[11],
            left: words[12],
            right: words[13],
            height: words[14],
            first: words[15] as usize,
            last: words[16] as usize,
            gap: words[17] as usize,
        }
    }

    fn write(self, address: u64) {
        let record = self.record;
        let journal = record.journal;
        let (mut physical, attributes) = journal.mapping.map_or((0, 0), |mapping| {
            let attributes = mapping.attributes;
            (
                mapping.physical_address,
                match attributes.permissions {
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
        };
        write(
            address,
            &[
                record.range.start as u64,
                record.range.end as u64,
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
                    | u64::from(journal.direct_was_writable) << 7,
                self.parent,
                self.left,
                self.right,
                self.height,
                self.first as u64,
                self.last as u64,
                self.gap as u64,
            ],
        );
    }
}

fn align(address: usize, alignment: usize) -> Option<usize> {
    address
        .checked_add(alignment - 1)
        .map(|value| value & !(alignment - 1))
}

fn read(address: u64, words: &mut [u64]) {
    hardware::mmu()
        .read_frame(
            address & !(PAGE_SIZE as u64 - 1),
            address as usize % PAGE_SIZE / 8,
            words,
        )
        .expect("failed to read mapping metadata");
}

fn write(address: u64, words: &[u64]) {
    hardware::mmu()
        .write_frame(
            address & !(PAGE_SIZE as u64 - 1),
            address as usize % PAGE_SIZE / 8,
            words,
        )
        .expect("failed to write mapping metadata");
}
