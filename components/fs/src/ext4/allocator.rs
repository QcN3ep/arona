// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! ext4 bitmap and free count updates.

use super::{half, volume::Volume, word};
use crate::Error;

impl Volume {
    pub fn allocate_block(&mut self) -> Result<u32, Error> {
        let hint_group = (self.allocation_hint.saturating_sub(self.first_data_block)
            / self.blocks_per_group) as usize
            % self.groups.len();
        for offset in 0..self.groups.len() {
            let group = (hint_group + offset) % self.groups.len();
            let first = self.group_start(group as u32);
            let end = first.saturating_add(self.blocks_per_group).min(self.blocks);
            let bitmap_number = self.groups[group].block_bitmap;
            let bitmap = self.block(bitmap_number)?;
            let hint = self.allocation_hint.clamp(first, end);
            for number in (hint..end).chain(first..hint) {
                let bit = (number - first) as usize;
                if bitmap[bit / 8] & (1 << (bit % 8)) == 0 && self.data_block(number) {
                    self.bitmap(bitmap_number, bit, true)?;
                    self.count(group, 12, 12, -1)?;
                    self.allocation_hint = (number + 1) % self.blocks;
                    let bytes = self.empty_block();
                    self.stage(number, bytes);
                    return Ok(number);
                }
            }
        }
        Err(Error::NoSpace)
    }

    pub fn free_block(&mut self, number: u32) -> Result<(), Error> {
        if !self.allocated_block(number)? {
            return Err(Error::InvalidData);
        }
        let group = self.group_index(number)? as usize;
        let first = self.group_start(group as u32);
        self.bitmap(
            self.groups[group].block_bitmap,
            (number - first) as usize,
            false,
        )?;
        self.count(group, 12, 12, 1)
    }

    pub fn allocate_inode(&mut self, directory: bool) -> Result<u32, Error> {
        let first_inode = self.superblock_word(84)?.max(11);
        for group in 0..self.groups.len() {
            let bitmap_number = self.groups[group].inode_bitmap;
            let bitmap = self.block(bitmap_number)?;
            for bit in 0..self.inodes_per_group {
                let number = group as u32 * self.inodes_per_group + bit + 1;
                if number > self.inodes {
                    break;
                }
                if number >= first_inode && bitmap[bit as usize / 8] & (1 << (bit % 8)) == 0 {
                    self.bitmap(bitmap_number, bit as usize, true)?;
                    self.count(group, 14, 16, -1)?;
                    if directory {
                        self.directory_count(group, 1)?;
                    }
                    return Ok(number);
                }
            }
        }
        Err(Error::NoSpace)
    }

    pub fn free_inode(&mut self, number: u32, directory: bool) -> Result<(), Error> {
        let group = ((number - 1) / self.inodes_per_group) as usize;
        self.bitmap(
            self.groups[group].inode_bitmap,
            ((number - 1) % self.inodes_per_group) as usize,
            false,
        )?;
        self.count(group, 14, 16, 1)?;
        if directory {
            self.directory_count(group, -1)?;
        }
        self.edit_inode(number, |bytes| {
            bytes.fill(0);
            Ok(())
        })
    }

    fn bitmap(&mut self, number: u32, bit: usize, allocated: bool) -> Result<(), Error> {
        self.edit(number, |bytes| {
            let mask = 1 << (bit % 8);
            if (bytes[bit / 8] & mask != 0) == allocated {
                return Err(Error::InvalidData);
            }
            if allocated {
                bytes[bit / 8] |= mask;
            } else {
                bytes[bit / 8] &= !mask;
            }
            Ok(())
        })
    }

    fn count(
        &mut self,
        group: usize,
        field: usize,
        sb_field: usize,
        change: i32,
    ) -> Result<(), Error> {
        let descriptor = self.first_data_block + 1 + (group as u32 / self.descriptors_per_block);
        let slot = (group as u32 % self.descriptors_per_block) as usize * 32;
        self.edit(descriptor, |bytes| {
            let offset = slot + field;
            let value = i32::from(half(bytes, offset))
                .checked_add(change)
                .and_then(|v| u16::try_from(v).ok())
                .ok_or(Error::InvalidData)?;
            bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
            Ok(())
        })?;
        self.edit_superblock(|bytes| {
            let value = i64::from(word(bytes, sb_field))
                .checked_add(i64::from(change))
                .and_then(|v| u32::try_from(v).ok())
                .ok_or(Error::InvalidData)?;
            bytes[sb_field..sb_field + 4].copy_from_slice(&value.to_le_bytes());
            Ok(())
        })
    }

    fn directory_count(&mut self, group: usize, change: i32) -> Result<(), Error> {
        let descriptor = self.first_data_block + 1 + (group as u32 / self.descriptors_per_block);
        let slot = (group as u32 % self.descriptors_per_block) as usize * 32;
        self.edit(descriptor, |bytes| {
            let offset = slot + 16;
            let count = i32::from(half(bytes, offset))
                .checked_add(change)
                .and_then(|n| u16::try_from(n).ok())
                .ok_or(Error::InvalidData)?;
            bytes[offset..offset + 2].copy_from_slice(&count.to_le_bytes());
            Ok(())
        })
    }
}
