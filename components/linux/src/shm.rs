// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! System V shared memory segments owned by the compatibility service.

use alloc::{collections::BTreeMap, rc::Rc};
use runtime::{FrameAllocation, Handle};

/// Segment limit reported by `IPC_INFO` and enforced on creation.
pub(crate) const MAX_SEGMENTS: usize = 4096;
/// Maximum segment size accepted by `shmget`, matching the reported `shmmax`.
pub(crate) const MAX_SIZE: usize = 1 << 30;

pub(crate) struct Table {
    segments: BTreeMap<usize, Segment>,
    keys: BTreeMap<i32, usize>,
    next: usize,
}

pub(crate) struct Segment {
    pub(crate) id: usize,
    pub(crate) key: Option<i32>,
    /// Requested byte count reported by `IPC_STAT` as `segsz`.
    pub(crate) size: usize,
    /// Page-rounded mapping length used for the frame count and `SHM_INFO`.
    pub(crate) mapped_length: usize,
    pub(crate) mode: u16,
    pub(crate) uid: u32,
    pub(crate) gid: u32,
    pub(crate) cuid: u32,
    pub(crate) cgid: u32,
    pub(crate) cpid: usize,
    pub(crate) lpid: usize,
    pub(crate) nattch: usize,
    pub(crate) atime: u64,
    pub(crate) dtime: u64,
    pub(crate) ctime: u64,
    pub(crate) removed: bool,
    pub(crate) frames: Option<Rc<Handle<FrameAllocation>>>,
}

/// An active attachment point recorded in a process address space.
#[derive(Clone, Copy)]
pub(crate) struct Attach {
    pub(crate) id: usize,
    pub(crate) mapped_length: usize,
    /// Inherited fork mappings share the page frames but are not counted separately.
    pub(crate) counted: bool,
}

impl Segment {
    /// Check requested permission bits (`rwx` repeated for owner, group, and other).
    pub(crate) fn permits(&self, uid: u32, gid: u32, requested: u16) -> bool {
        if uid == 0 {
            return true;
        }
        let (granted, requested) = if uid == self.uid {
            ((self.mode >> 6) & 7, (requested >> 6) & 7)
        } else if gid == self.gid {
            ((self.mode >> 3) & 7, (requested >> 3) & 7)
        } else {
            (self.mode & 7, requested & 7)
        };
        requested & !granted == 0
    }
}

impl Table {
    pub(crate) fn new() -> Self {
        Self {
            segments: BTreeMap::new(),
            keys: BTreeMap::new(),
            next: 1,
        }
    }

    pub(crate) fn by_key(&self, key: i32) -> Option<&Segment> {
        self.keys.get(&key).and_then(|id| self.segments.get(id))
    }

    pub(crate) fn get(&self, id: usize) -> Option<&Segment> {
        self.segments.get(&id)
    }

    pub(crate) fn get_mut(&mut self, id: usize) -> Option<&mut Segment> {
        self.segments.get_mut(&id)
    }

    /// Record one attachment and its last attaching process.
    pub(crate) fn attach(&mut self, id: usize, pid: usize, now: u64) {
        if let Some(segment) = self.segments.get_mut(&id) {
            segment.nattch = segment.nattch.saturating_add(1);
            segment.atime = now;
            segment.lpid = pid;
        }
    }

    /// Mark a segment removed and destroy it when no attachment remains.
    ///
    /// The key is released immediately so a later `shmget` cannot find it,
    /// while existing attachments keep the segment until the last detach.
    pub(crate) fn mark_removed(&mut self, id: usize, pid: usize, now: u64) {
        let destroy = match self.segments.get_mut(&id) {
            Some(segment) => {
                segment.removed = true;
                segment.ctime = now;
                segment.lpid = pid;
                segment.nattch == 0
            }
            None => return,
        };
        self.detach_key(id);
        if destroy {
            self.remove(id);
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.segments.len()
    }

    pub(crate) fn max_id(&self) -> usize {
        self.segments.keys().next_back().copied().unwrap_or(0)
    }

    pub(crate) fn total_size(&self) -> usize {
        self.segments
            .values()
            .map(|segment| segment.mapped_length)
            .sum()
    }

    /// Reserve a fresh identifier. Identifiers are never reused while allocated.
    pub(crate) fn allocate_id(&mut self) -> Option<usize> {
        let start = self.next;
        loop {
            let id = self.next;
            self.next = self.next.checked_add(1).unwrap_or(1);
            if !self.segments.contains_key(&id) {
                return Some(id);
            }
            if self.next == start {
                return None;
            }
        }
    }

    pub(crate) fn insert(&mut self, segment: Segment) {
        if let Some(key) = segment.key {
            self.keys.insert(key, segment.id);
        }
        self.segments.insert(segment.id, segment);
    }

    /// Remove a segment from key lookup while keeping it alive for existing users.
    pub(crate) fn detach_key(&mut self, id: usize) {
        if let Some(segment) = self.segments.get_mut(&id)
            && let Some(key) = segment.key.take()
            && self.keys.get(&key) == Some(&id)
        {
            self.keys.remove(&key);
        }
    }

    pub(crate) fn remove(&mut self, id: usize) {
        if let Some(segment) = self.segments.remove(&id)
            && let Some(key) = segment.key
            && self.keys.get(&key) == Some(&id)
        {
            self.keys.remove(&key);
        }
    }

    /// Account one detachment and destroy a removed segment after its last attachment.
    pub(crate) fn detach(&mut self, id: usize, pid: usize, now: u64) {
        let destroyed = match self.segments.get_mut(&id) {
            Some(segment) => {
                segment.nattch = segment.nattch.saturating_sub(1);
                segment.lpid = pid;
                segment.dtime = now;
                segment.removed && segment.nattch == 0
            }
            None => false,
        };
        if destroyed {
            self.remove(id);
        }
    }
}
