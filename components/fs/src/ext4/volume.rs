// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::FileIdentity;
use super::{half, inode::Inode, journal::Journal, word};
use crate::{
    Error,
    block::{Disk, MAX_BLOCK_SIZE},
};
use alloc::{
    collections::BTreeMap,
    rc::{Rc, Weak},
    vec::Vec,
};

pub(super) const SUPERBLOCK_OFFSET: usize = 1024;
pub(super) const SUPERBLOCK_SIZE: usize = 1024;

pub(super) struct Volume {
    pub device: u64,
    pub mount: u64,
    pub disk: Rc<Disk>,
    pub start: u64,
    pub block_size: usize,
    pub first_data_block: u32,
    pub descriptors_per_block: u32,
    pub descriptor_blocks: u32,
    pub(super) dirty: bool,
    pub(super) closed: bool,
    pub(super) journal: Option<Journal>,
    pub blocks: u32,
    reserved_blocks: u32,
    pub(super) allocation_hint: u32,
    pub inodes: u32,
    pub blocks_per_group: u32,
    pub inodes_per_group: u32,
    pub inode_size: usize,
    pub groups: Vec<Group>,
    sparse: bool,
    pub(super) cache: BTreeMap<u32, Rc<[u8; MAX_BLOCK_SIZE]>>,
    pub(super) pending: BTreeMap<u32, Rc<[u8; MAX_BLOCK_SIZE]>>,
    pub(super) failure: Option<Error>,
    pub(super) open: BTreeMap<u32, Weak<FileIdentity>>,
}

pub(super) struct Group {
    pub block_bitmap: u32,
    pub inode_bitmap: u32,
    pub inode_table: u32,
}

struct Superblock {
    block_size: usize,
    first_data_block: u32,
    blocks: u32,
    inodes: u32,
    blocks_per_group: u32,
    inodes_per_group: u32,
    inode_size: usize,
    sparse: bool,
}

impl Superblock {
    fn parse(sb: &[u8], length: u64) -> Result<Self, Error> {
        let log_block_size = word(sb, 24);
        if log_block_size > 2
            || half(sb, 56) != 0xef53
            || word(sb, 20) != u32::from(log_block_size == 0)
            || word(sb, 28) != log_block_size
            || word(sb, 76) != 1
            || word(sb, 92) & !4 != 0
            || word(sb, 96) & !0x46 != 0
            || word(sb, 96) & 0x42 != 0x42
            || word(sb, 100) & !3 != 0
            || half(sb, 58) & !1 != 0
            || word(sb, 336) != 0
            || !matches!(half(sb, 88), 128 | 256)
        {
            return Err(Error::Unsupported);
        }
        let block_size = 1024usize << log_block_size;
        let first_data_block = word(sb, 20);
        let blocks = word(sb, 4);
        let inodes = word(sb, 0);
        let blocks_per_group = word(sb, 32);
        let inodes_per_group = word(sb, 40);
        if blocks <= first_data_block
            || u64::from(blocks) * block_size as u64 > length
            || inodes < 11
            || blocks_per_group == 0
            || u64::from(blocks_per_group) > block_size as u64 * 8
            || inodes_per_group == 0
            || u64::from(inodes_per_group) > block_size as u64 * 8
        {
            return Err(Error::InvalidData);
        }
        Ok(Self {
            block_size,
            first_data_block,
            blocks,
            inodes,
            blocks_per_group,
            inodes_per_group,
            inode_size: usize::from(half(sb, 88)),
            sparse: word(sb, 100) & 1 != 0,
        })
    }
}

impl Volume {
    // TODO: Extend ext4 disk feature support beyond block sizes 1024, 2048, and 4096.
    // REFERENCE: https://www.kernel.org/doc/html/v6.12/filesystems/ext4/super.html
    pub fn mount(disk: Rc<Disk>, start: u64, length: u64) -> Result<Self, Error> {
        if !start.is_multiple_of(512)
            || length < MAX_BLOCK_SIZE as u64
            || start
                .checked_add(length)
                .is_none_or(|end| end > disk.capacity())
        {
            return Err(Error::InvalidData);
        }
        let mut block = [0; MAX_BLOCK_SIZE];
        disk.read(start, &mut block)?;
        let sb = &block[SUPERBLOCK_OFFSET..SUPERBLOCK_OFFSET + SUPERBLOCK_SIZE];
        let superblock = Superblock::parse(sb, length)?;
        if !start.is_multiple_of(superblock.block_size as u64) {
            return Err(Error::InvalidData);
        }
        let count =
            (superblock.blocks - superblock.first_data_block).div_ceil(superblock.blocks_per_group);
        if superblock.inodes.div_ceil(superblock.inodes_per_group) != count {
            return Err(Error::InvalidData);
        }
        let descriptors_per_block = (superblock.block_size / 32) as u32;
        let descriptor_blocks =
            (u64::from(count) * 32).div_ceil(superblock.block_size as u64) as u32;
        let device = crate::metadata::device_identity()?;
        let mount = crate::metadata::mount_identity()?;
        disk.claim(start, length)?;
        let mut volume = Self {
            device,
            mount,
            disk,
            start,
            block_size: superblock.block_size,
            first_data_block: superblock.first_data_block,
            descriptors_per_block,
            descriptor_blocks,
            dirty: false,
            closed: false,
            journal: None,
            blocks: superblock.blocks,
            reserved_blocks: word(sb, 8),
            allocation_hint: 0,
            inodes: superblock.inodes,
            blocks_per_group: superblock.blocks_per_group,
            inodes_per_group: superblock.inodes_per_group,
            inode_size: superblock.inode_size,
            groups: Vec::new(),
            sparse: superblock.sparse,
            cache: BTreeMap::new(),
            pending: BTreeMap::new(),
            failure: None,
            open: BTreeMap::new(),
        };
        volume.load_groups()?;
        volume.load_journal(sb)?;
        let root = volume.inode(2)?;
        if !root.directory() {
            return Err(Error::InvalidData);
        }
        volume.entries(&root)?;
        volume.mark_clean()?;
        Ok(volume)
    }

    pub(super) fn superblock_offset(&self) -> usize {
        SUPERBLOCK_OFFSET % self.block_size
    }

    pub(super) fn superblock(&mut self) -> Result<Rc<[u8; MAX_BLOCK_SIZE]>, Error> {
        self.block(self.first_data_block)
    }

    pub(super) fn superblock_word(&mut self, offset: usize) -> Result<u32, Error> {
        let base = self.superblock_offset();
        Ok(word(&self.superblock()?[..], base + offset))
    }

    pub(super) fn edit_superblock(
        &mut self,
        operation: impl FnOnce(&mut [u8]) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let base = self.superblock_offset();
        self.edit(self.first_data_block, |bytes| {
            operation(&mut bytes[base..base + SUPERBLOCK_SIZE])
        })
    }

    pub(super) fn empty_block(&self) -> Rc<[u8; MAX_BLOCK_SIZE]> {
        Rc::new([0; MAX_BLOCK_SIZE])
    }

    pub(super) fn group_start(&self, index: u32) -> u32 {
        self.first_data_block + index * self.blocks_per_group
    }

    pub(super) fn group_index(&self, number: u32) -> Result<u32, Error> {
        number
            .checked_sub(self.first_data_block)
            .map(|offset| offset / self.blocks_per_group)
            .ok_or(Error::InvalidData)
    }

    fn load_groups(&mut self) -> Result<(), Error> {
        let count = self
            .blocks
            .saturating_sub(self.first_data_block)
            .div_ceil(self.blocks_per_group);
        let tables = (u64::from(self.inodes_per_group) * self.inode_size as u64)
            .div_ceil(self.block_size as u64) as u32;
        for index in 0..count {
            let descriptor =
                self.block(self.first_data_block + 1 + index / self.descriptors_per_block)?;
            let offset = (index % self.descriptors_per_block) as usize * 32;
            let group = Group {
                block_bitmap: word(&descriptor[..], offset),
                inode_bitmap: word(&descriptor[..], offset + 4),
                inode_table: word(&descriptor[..], offset + 8),
            };
            let first = self.group_start(index);
            let last = first.saturating_add(self.blocks_per_group).min(self.blocks);
            let reserved_end = first
                + if self.has_super(index) {
                    1 + self.descriptor_blocks
                } else {
                    0
                };
            if last <= first
                || half(&descriptor[..], offset + 18) & !4 != 0
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

    pub fn statistics(&mut self) -> Result<crate::Statistics, Error> {
        let mut free_blocks = 0;
        let mut free_files = 0;
        for index in 0..self.groups.len() {
            let bitmap = self.block(self.groups[index].block_bitmap)?;
            free_blocks += count_free(
                &bitmap[..],
                index,
                self.blocks_per_group,
                self.blocks - self.first_data_block,
            );
            let bitmap = self.block(self.groups[index].inode_bitmap)?;
            free_files += count_free(&bitmap[..], index, self.inodes_per_group, self.inodes);
        }
        Ok(crate::Statistics {
            kind: 0xef53,
            device: self.device,
            block_size: self.block_size as u64,
            blocks: u64::from(self.blocks),
            free_blocks,
            available_blocks: free_blocks.saturating_sub(u64::from(self.reserved_blocks)),
            files: u64::from(self.inodes),
            free_files,
            name_length: 255,
            flags: u64::from(self.disk.read_only() || self.journal.is_none()),
        })
    }

    pub fn block(&mut self, number: u32) -> Result<Rc<[u8; MAX_BLOCK_SIZE]>, Error> {
        if self.closed {
            return Err(Error::Unavailable);
        }
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
        let mut bytes = [0; MAX_BLOCK_SIZE];
        self.disk.read(
            self.start + u64::from(number) * self.block_size as u64,
            &mut bytes[..self.block_size],
        )?;
        if self.cache.len() == 64 {
            self.cache.pop_first();
        }
        let bytes = Rc::new(bytes);
        self.cache.insert(number, Rc::clone(&bytes));
        Ok(bytes)
    }

    pub fn data_block(&self, number: u32) -> bool {
        if number < self.first_data_block
            || number >= self.blocks
            || self
                .journal
                .as_ref()
                .is_some_and(|journal| journal.contains(number))
        {
            return false;
        }
        let index = (number - self.first_data_block) / self.blocks_per_group;
        let Some(group) = self.groups.get(index as usize) else {
            return false;
        };
        let first = self.group_start(index);
        let tables = (u64::from(self.inodes_per_group) * self.inode_size as u64)
            .div_ceil(self.block_size as u64) as u32;
        let reserved_end = first
            + if self.has_super(index) {
                1 + self.descriptor_blocks
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
        let group = self.group_index(number)?;
        let first = self.group_start(group);
        let bit = number - first;
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
        let bytes = self
            .block(self.groups[group as usize].inode_table + (offset / self.block_size) as u32)?;
        Inode::parse(
            number,
            &bytes[offset % self.block_size..offset % self.block_size + self.inode_size],
        )
    }
}

impl Drop for Volume {
    fn drop(&mut self) {
        if !self.closed {
            self.disk.release(self.start);
        }
    }
}

fn count_free(bitmap: &[u8], index: usize, per_group: u32, total: u32) -> u64 {
    let start = (index as u64 * u64::from(per_group)).min(u64::from(total));
    let count = (u64::from(total) - start).min(u64::from(per_group));
    (0..count)
        .filter(|bit| bitmap[*bit as usize / 8] >> (*bit as usize % 8) & 1 == 0)
        .count() as u64
}
