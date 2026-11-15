// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Block group layouts, lazy bitmap initialization, and descriptor checksums.

use super::{
    half,
    journal::crc32c,
    volume::{Group, Volume},
    word,
};
use crate::{Error, block::MAX_BLOCK_SIZE};
use alloc::rc::Rc;

impl Volume {
    fn descriptor(&mut self, index: usize) -> Result<(Rc<[u8; MAX_BLOCK_SIZE]>, usize), Error> {
        let number = self.first_data_block + 1 + index as u32 / self.descriptors_per_block;
        let slot = (index as u32 % self.descriptors_per_block) as usize * self.descriptor_size;
        let bytes = self.block(number)?;
        let descriptor = &bytes[slot..slot + self.descriptor_size];
        if self.checksum.enabled
            && self.checksum.group(index as u32, descriptor) != half(descriptor, 30)
        {
            return Err(Error::InvalidData);
        }
        Ok((bytes, slot))
    }

    pub(super) fn edit_group(
        &mut self,
        index: usize,
        operation: impl FnOnce(&mut [u8]) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let (_, slot) = self.descriptor(index)?;
        let number = self.first_data_block + 1 + index as u32 / self.descriptors_per_block;
        let size = self.descriptor_size;
        let checksum = self.checksum;
        self.edit(number, |bytes| {
            let descriptor = &mut bytes[slot..slot + size];
            operation(descriptor)?;
            if checksum.enabled {
                let sum = checksum.group(index as u32, descriptor);
                descriptor[30..32].copy_from_slice(&sum.to_le_bytes());
            }
            Ok(())
        })
    }

    pub(super) fn load_groups(&mut self) -> Result<(), Error> {
        self.groups.clear();
        self.reserved.clear();
        let count = (self.blocks - self.first_data_block).div_ceil(self.blocks_per_group);
        let tables = (u64::from(self.inodes_per_group) * self.inode_size as u64)
            .div_ceil(self.block_size as u64) as u32;
        for index in 0..count {
            let (bytes, slot) = self.descriptor(index as usize)?;
            let bytes = &bytes[slot..slot + self.descriptor_size];
            if half(bytes, 18) & !7 != 0
                || !self.checksum.enabled && half(bytes, 18) & 3 != 0
                || index == 0 && half(bytes, 18) & 3 != 0
                || self.descriptor_size == 64
                    && ([32, 36, 40].iter().any(|offset| word(bytes, *offset) != 0)
                        || [44, 46, 48, 50]
                            .iter()
                            .any(|offset| half(bytes, *offset) != 0))
            {
                return Err(Error::Unsupported);
            }
            let group = Group {
                block_bitmap: word(bytes, 0),
                inode_bitmap: word(bytes, 4),
                inode_table: word(bytes, 8),
            };
            self.reserved.push(
                group.block_bitmap
                    ..group
                        .block_bitmap
                        .checked_add(1)
                        .ok_or(Error::InvalidData)?,
            );
            self.reserved.push(
                group.inode_bitmap
                    ..group
                        .inode_bitmap
                        .checked_add(1)
                        .ok_or(Error::InvalidData)?,
            );
            self.reserved.push(
                group.inode_table
                    ..group
                        .inode_table
                        .checked_add(tables)
                        .ok_or(Error::InvalidData)?,
            );
            if self.has_super(index) {
                let start = self.group_start(index);
                let end = start
                    .checked_add(1 + self.descriptor_blocks + u32::from(self.reserved_gdt))
                    .ok_or(Error::InvalidData)?;
                if end > start.saturating_add(self.blocks_per_group).min(self.blocks) {
                    return Err(Error::InvalidData);
                }
                self.reserved.push(start..end);
            }
            self.groups.push(group);
        }
        self.reserved.sort_unstable_by_key(|range| range.start);
        if self
            .reserved
            .iter()
            .any(|range| range.start < self.first_data_block || range.end > self.blocks)
            || self
                .reserved
                .windows(2)
                .any(|pair| pair[0].end > pair[1].start)
        {
            return Err(Error::InvalidData);
        }
        Ok(())
    }

    pub(super) fn group_bitmap(
        &mut self,
        index: usize,
        inode: bool,
    ) -> Result<Rc<[u8; MAX_BLOCK_SIZE]>, Error> {
        let (descriptor, slot) = self.descriptor(index)?;
        let descriptor = &descriptor[slot..slot + self.descriptor_size];
        let flag = if inode { 1 } else { 2 };
        let count = if inode {
            self.inodes_per_group
        } else {
            self.blocks_per_group
        };
        if half(descriptor, 18) & flag != 0 {
            let mut bitmap = self.empty_block();
            let bytes = Rc::make_mut(&mut bitmap);
            let first = if inode {
                index as u32 * count
            } else {
                self.group_start(index as u32)
            };
            let total = if inode { self.inodes } else { self.blocks };
            for bit in 0..self.block_size * 8 {
                let number = u64::from(first) + bit as u64;
                if bit >= count as usize
                    || number >= u64::from(total)
                    || !inode && !self.data_block(number as u32)
                {
                    bytes[bit / 8] |= 1 << (bit % 8);
                }
            }
            return Ok(bitmap);
        }
        let group = &self.groups[index];
        let number = if inode {
            group.inode_bitmap
        } else {
            group.block_bitmap
        };
        let bitmap = self.block(number)?;
        if self.checksum.enabled
            && !self
                .verified_bitmaps
                .get(&number)
                .is_some_and(|verified| verified.ptr_eq(&Rc::downgrade(&bitmap)))
        {
            let low = if inode { 26 } else { 24 };
            let high = if inode { 58 } else { 56 };
            let mut expected = u32::from(half(descriptor, low));
            let mut sum = crc32c(self.checksum.seed, &bitmap[..count as usize / 8]);
            if self.descriptor_size == 64 {
                expected |= u32::from(half(descriptor, high)) << 16;
            } else {
                sum &= 0xffff;
            }
            if sum != expected {
                return Err(Error::InvalidData);
            }
            self.verified_bitmaps.insert(number, Rc::downgrade(&bitmap));
        }
        Ok(bitmap)
    }

    pub(super) fn update_bitmap(
        &mut self,
        index: usize,
        inode: bool,
        bit: usize,
        allocated: bool,
    ) -> Result<(), Error> {
        let mut bitmap = self.group_bitmap(index, inode)?;
        let bytes = Rc::make_mut(&mut bitmap);
        let mask = 1 << (bit % 8);
        if (bytes[bit / 8] & mask != 0) == allocated {
            return Err(Error::InvalidData);
        }
        if allocated {
            bytes[bit / 8] |= mask;
        } else {
            bytes[bit / 8] &= !mask;
        }
        let count = if inode {
            self.inodes_per_group
        } else {
            self.blocks_per_group
        };
        let sum = crc32c(self.checksum.seed, &bytes[..count as usize / 8]);
        let enabled = self.checksum.enabled;
        let group = &self.groups[index];
        self.stage(
            if inode {
                group.inode_bitmap
            } else {
                group.block_bitmap
            },
            bitmap,
        );
        self.edit_group(index, |bytes| {
            let flags = half(bytes, 18) & !if inode { 1 } else { 2 };
            bytes[18..20].copy_from_slice(&flags.to_le_bytes());
            if enabled {
                let low = if inode { 26 } else { 24 };
                let high = if inode { 58 } else { 56 };
                bytes[low..low + 2].copy_from_slice(&(sum as u16).to_le_bytes());
                if bytes.len() == 64 {
                    bytes[high..high + 2].copy_from_slice(&((sum >> 16) as u16).to_le_bytes());
                }
                if inode && allocated {
                    let unused = u32::from(half(bytes, 28)).min(count - bit as u32 - 1);
                    bytes[28..30].copy_from_slice(&(unused as u16).to_le_bytes());
                }
            }
            Ok(())
        })
    }
}
