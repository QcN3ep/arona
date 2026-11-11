// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Mount recovery and orphan inode reclamation.

use super::{half, inode::Inode, journal::Journal, volume::Volume, word};
use crate::{Error, block::BLOCK_SIZE};
use alloc::{collections::BTreeSet, vec::Vec};

impl Volume {
    pub fn load_journal(&mut self, sb: &[u8]) -> Result<(), Error> {
        let recover = word(sb, 96) & 4 != 0;
        if word(sb, 92) & 4 == 0 {
            if recover || half(sb, 58) != 1 || word(sb, 232) != 0 {
                return Err(Error::Unsupported);
            }
            return Ok(());
        }
        if word(sb, 224) != 8 || word(sb, 228) != 0 || half(sb, 58) != 1 && !recover {
            return Err(Error::Unsupported);
        }
        let inode = self.inode(8)?;
        if inode.directory() || !inode.size.is_multiple_of(BLOCK_SIZE as u64) {
            return Err(Error::InvalidData);
        }
        let mut locations = Vec::new();
        for extent in self.extents(&inode.root)? {
            if extent.unwritten || extent.logical as usize != locations.len() {
                return Err(Error::InvalidData);
            }
            locations.extend(extent.physical..extent.physical + extent.length);
        }
        if locations.len() as u64 * BLOCK_SIZE as u64 != inode.size {
            return Err(Error::InvalidData);
        }
        let nodes = self.extent_blocks(&inode.root)?;
        let mut journal = Journal::open(&self.disk, self.start, locations, nodes, &sb[104..120])?;
        if !recover && journal.pending() {
            return Err(Error::InvalidData);
        }
        if recover {
            if self.disk.read_only() || !self.disk.supports_flush() {
                return Err(Error::ReadOnly);
            }
            journal.recover(&self.disk, self.start, self.blocks)?;
            self.cache.clear();
        }
        self.journal = Some(journal);
        if recover {
            self.dirty = true;
            self.recover_orphans()?;
        } else if word(sb, 232) != 0 {
            return Err(Error::InvalidData);
        }
        Ok(())
    }

    // REFERENCE: https://www.kernel.org/doc/html/v6.12/filesystems/ext4/orphan.html
    pub fn add_orphan(&mut self, inode: &Inode) -> Result<(), Error> {
        let next = word(&self.block(0)?[..], 1024 + 232);
        self.edit_inode(inode.number, |bytes| {
            bytes[20..24].copy_from_slice(&next.to_le_bytes());
            Ok(())
        })?;
        self.edit(0, |bytes| {
            bytes[1024 + 232..1024 + 236].copy_from_slice(&inode.number.to_le_bytes());
            Ok(())
        })
    }

    pub fn remove_orphan(&mut self, inode: &Inode) -> Result<(), Error> {
        let mut number = word(&self.block(0)?[..], 1024 + 232);
        let mut previous = None;
        let mut visited = BTreeSet::new();
        while number != 0 {
            if !visited.insert(number) {
                return Err(Error::InvalidData);
            }
            if number == inode.number {
                return if let Some(previous) = previous {
                    self.edit_inode(previous, |bytes| {
                        bytes[20..24].copy_from_slice(&inode.orphan_next.to_le_bytes());
                        Ok(())
                    })
                } else {
                    self.edit(0, |bytes| {
                        bytes[1256..1260].copy_from_slice(&inode.orphan_next.to_le_bytes());
                        Ok(())
                    })
                };
            }
            previous = Some(number);
            number = self.inode(number)?.orphan_next;
        }
        Ok(())
    }

    fn recover_orphans(&mut self) -> Result<(), Error> {
        let mut visited = BTreeSet::new();
        let mut orphans = Vec::new();
        let mut number = word(&self.block(0)?[..], 1256);
        while number != 0 {
            if number < 11 || !visited.insert(number) {
                return Err(Error::InvalidData);
            }
            let inode = self.inode(number)?;
            if inode.links != 0 {
                return Err(Error::Unsupported);
            }
            number = inode.orphan_next;
            orphans.push(inode);
        }
        for mut inode in orphans {
            self.transaction(|volume| {
                volume.remove_orphan(&inode)?;
                volume.destroy_inode(&mut inode)
            })?;
        }
        Ok(())
    }
}
