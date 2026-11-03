// SPDX-License-Identifier: MPL-2.0

use super::FileIdentity;
use super::{half, inode::Inode, word};
use crate::{
    Error,
    block::{BLOCK_SIZE, Disk},
};
use alloc::{
    collections::BTreeMap,
    rc::{Rc, Weak},
    vec::Vec,
};

pub(super) struct Volume {
    pub disk: Rc<Disk>,
    pub start: u64,
    pub blocks: u32,
    pub(super) allocation_hint: u32,
    pub inodes: u32,
    pub blocks_per_group: u32,
    pub inodes_per_group: u32,
    pub inode_size: usize,
    pub groups: Vec<Group>,
    sparse: bool,
    pub(super) cache: BTreeMap<u32, Rc<[u8; BLOCK_SIZE]>>,
    pub(super) pending: BTreeMap<u32, Rc<[u8; BLOCK_SIZE]>>,
    pub(super) failure: Option<Error>,
    pub(super) open: BTreeMap<u32, Weak<FileIdentity>>,
}

pub(super) struct Group {
    pub block_bitmap: u32,
    pub inode_bitmap: u32,
    pub inode_table: u32,
}

impl Volume {
    // REFERENCE: https://www.kernel.org/doc/html/v6.12/filesystems/ext4/super.html
    pub fn mount(disk: Rc<Disk>, start: u64, length: u64) -> Result<Self, Error> {
        if !start.is_multiple_of(BLOCK_SIZE as u64)
            || length < 3 * BLOCK_SIZE as u64
            || start
                .checked_add(length)
                .is_none_or(|end| end > disk.capacity())
        {
            return Err(Error::InvalidData);
        }
        let mut block = [0; BLOCK_SIZE];
        disk.read(start, &mut block)?;
        let sb = &block[1024..2048];
        if half(sb, 56) != 0xef53
            || word(sb, 24) != 2
            || word(sb, 28) != 2
            || word(sb, 20) != 0
            || word(sb, 76) != 1
            || word(sb, 92) != 0
            || word(sb, 96) & !0x42 != 0
            || word(sb, 96) & 0x42 != 0x42
            || word(sb, 100) & !3 != 0
            || half(sb, 58) != 1
            || word(sb, 232) != 0
            || word(sb, 336) != 0
            || !matches!(half(sb, 88), 128 | 256)
        {
            return Err(Error::Unsupported);
        }
        let blocks = word(sb, 4);
        let inodes = word(sb, 0);
        let blocks_per_group = word(sb, 32);
        let inodes_per_group = word(sb, 40);
        if blocks < 3
            || u64::from(blocks) * BLOCK_SIZE as u64 > length
            || inodes < 11
            || blocks_per_group == 0
            || blocks_per_group > 32768
            || inodes_per_group == 0
            || inodes_per_group > 32768
        {
            return Err(Error::InvalidData);
        }
        let count = blocks.div_ceil(blocks_per_group);
        if inodes.div_ceil(inodes_per_group) != count {
            return Err(Error::InvalidData);
        }
        let mut volume = Self {
            disk,
            start,
            blocks,
            allocation_hint: 0,
            inodes,
            blocks_per_group,
            inodes_per_group,
            inode_size: usize::from(half(sb, 88)),
            groups: Vec::new(),
            sparse: word(sb, 100) & 1 != 0,
            cache: BTreeMap::new(),
            pending: BTreeMap::new(),
            failure: None,
            open: BTreeMap::new(),
        };
        volume.load_groups()?;
        Ok(volume)
    }

    fn load_groups(&mut self) -> Result<(), Error> {
        for index in 0..self.blocks.div_ceil(self.blocks_per_group) {
            let descriptor = self.block(1 + index / 128)?;
            let offset = (index as usize % 128) * 32;
            let group = Group {
                block_bitmap: word(&descriptor[..], offset),
                inode_bitmap: word(&descriptor[..], offset + 4),
                inode_table: word(&descriptor[..], offset + 8),
            };
            let first = index * self.blocks_per_group;
            let last = first.saturating_add(self.blocks_per_group).min(self.blocks);
            let tables = (u64::from(self.inodes_per_group) * self.inode_size as u64)
                .div_ceil(BLOCK_SIZE as u64) as u32;
            let reserved_end = first
                + if self.has_super(index) {
                    1 + self.blocks.div_ceil(self.blocks_per_group).div_ceil(128)
                } else {
                    0
                };
            if half(&descriptor[..], offset + 18) & !4 != 0
                || group.block_bitmap < reserved_end
                || group.block_bitmap >= last
                || group.inode_bitmap < reserved_end
                || group.inode_bitmap >= last
                || group.inode_table < reserved_end
                || group
                    .inode_table
                    .checked_add(tables)
                    .is_none_or(|end| end > last)
                || group.block_bitmap == group.inode_bitmap
                || (group.inode_table..group.inode_table + tables).contains(&group.block_bitmap)
                || (group.inode_table..group.inode_table + tables).contains(&group.inode_bitmap)
            {
                return Err(Error::InvalidData);
            }
            self.groups.push(group);
        }
        Ok(())
    }

    pub fn block(&mut self, number: u32) -> Result<Rc<[u8; BLOCK_SIZE]>, Error> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if number >= self.blocks {
            return Err(Error::InvalidData);
        }
        if let Some(bytes) = self.pending.get(&number) {
            return Ok(Rc::clone(bytes));
        }
        if let Some(bytes) = self.cache.get(&number) {
            return Ok(Rc::clone(bytes));
        }
        let mut bytes = [0; BLOCK_SIZE];
        self.disk.read(
            self.start + u64::from(number) * BLOCK_SIZE as u64,
            &mut bytes,
        )?;
        if self.cache.len() == 64 {
            self.cache.pop_first();
        }
        let bytes = Rc::new(bytes);
        self.cache.insert(number, Rc::clone(&bytes));
        Ok(bytes)
    }

    pub fn data_block(&self, number: u32) -> bool {
        if number >= self.blocks {
            return false;
        }
        let index = number / self.blocks_per_group;
        let group = &self.groups[index as usize];
        let first = index * self.blocks_per_group;
        let tables = (u64::from(self.inodes_per_group) * self.inode_size as u64)
            .div_ceil(BLOCK_SIZE as u64) as u32;
        let reserved_end = first
            + if self.has_super(index) {
                1 + (self.groups.len() as u32).div_ceil(128)
            } else {
                0
            };
        number >= reserved_end
            && number != group.block_bitmap
            && number != group.inode_bitmap
            && !(group.inode_table..group.inode_table + tables).contains(&number)
    }

    fn has_super(&self, group: u32) -> bool {
        if !self.sparse || group <= 1 {
            return true;
        }
        [3, 5, 7].iter().any(|base| {
            let mut power = *base;
            while power < group {
                let Some(next) = power.checked_mul(*base) else {
                    break;
                };
                power = next;
            }
            power == group
        })
    }

    pub fn allocated_block(&mut self, number: u32) -> Result<bool, Error> {
        if !self.data_block(number) {
            return Ok(false);
        }
        let group = number / self.blocks_per_group;
        let bit = number % self.blocks_per_group;
        let bitmap = self.block(self.groups[group as usize].block_bitmap)?;
        Ok(bitmap[bit as usize / 8] & (1 << (bit % 8)) != 0)
    }

    pub fn inode(&mut self, number: u32) -> Result<Inode, Error> {
        if number == 0 || number > self.inodes {
            return Err(Error::InvalidData);
        }
        let group = (number - 1) / self.inodes_per_group;
        let bit = (number - 1) % self.inodes_per_group;
        let bitmap = self.block(self.groups[group as usize].inode_bitmap)?;
        if bitmap[bit as usize / 8] & (1 << (bit % 8)) == 0 {
            return Err(Error::InvalidData);
        }
        let offset = bit as usize * self.inode_size;
        let bytes =
            self.block(self.groups[group as usize].inode_table + (offset / BLOCK_SIZE) as u32)?;
        Inode::parse(
            number,
            &bytes[offset % BLOCK_SIZE..offset % BLOCK_SIZE + self.inode_size],
        )
    }
}
