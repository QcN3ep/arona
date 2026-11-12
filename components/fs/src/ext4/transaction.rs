// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! ext4 update staging and write failure state.

use super::{inode::Inode, volume::Volume};
use crate::{Error, block::MAX_BLOCK_SIZE};
use alloc::rc::Rc;

impl Volume {
    pub fn check_writable(&self) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Unavailable);
        }
        if let Some(error) = self.failure {
            return Err(error);
        }
        if self.disk.read_only() || self.journal.is_none() {
            return Err(Error::ReadOnly);
        }
        Ok(())
    }

    pub fn transaction<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<T, Error>,
    ) -> Result<T, Error> {
        self.check_writable()?;
        let value = match operation(self) {
            Ok(value) => value,
            Err(error) => {
                self.pending.clear();
                return Err(error);
            }
        };
        if let Err(error) = self
            .journal
            .as_ref()
            .unwrap()
            .check_capacity(self.pending.len())
        {
            self.pending.clear();
            return Err(error);
        }
        if let Err(error) = self.begin_update() {
            self.pending.clear();
            return Err(error);
        }
        let pending = core::mem::take(&mut self.pending);
        if let Err(error) = self
            .journal
            .as_mut()
            .unwrap()
            .commit(&self.disk, self.start, &pending)
        {
            self.failure = Some(error);
            self.cache.clear();
            return Err(error);
        }
        for (number, bytes) in pending {
            if self.cache.len() >= 64 {
                self.cache.pop_first();
            }
            self.cache.insert(number, bytes);
        }
        Ok(value)
    }

    fn begin_update(&mut self) -> Result<(), Error> {
        if !self.disk.supports_flush() {
            return Err(Error::Unsupported);
        }
        if !self.dirty {
            let superblock = self.first_data_block;
            let staged = self.pending.remove(&superblock);
            let original = self.block(superblock);
            if let Some(bytes) = staged {
                self.pending.insert(superblock, bytes);
            }
            let mut bytes = original?;
            update_superblock_state(&mut bytes, self.superblock_offset(), false);
            let result = self
                .disk
                .write(
                    self.start + u64::from(superblock) * self.block_size as u64,
                    &bytes[..self.block_size],
                )
                .and_then(|_| self.disk.flush());
            if let Err(error) = result {
                self.failure = Some(error);
                return Err(error);
            }
            self.cache.insert(superblock, bytes);
            self.dirty = true;
        }
        let superblock = self.first_data_block;
        let offset = self.superblock_offset();
        if let Some(bytes) = self.pending.get_mut(&superblock) {
            update_superblock_state(bytes, offset, false);
        }
        Ok(())
    }

    pub fn sync(&mut self) -> Result<(), Error> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if self.closed {
            return Err(Error::Unavailable);
        }
        if self.dirty
            && let Err(error) = self.disk.flush()
        {
            self.failure = Some(error);
            return Err(error);
        }
        Ok(())
    }

    pub fn unmount(&mut self) -> Result<(), Error> {
        self.sync()?;
        self.mark_clean()?;
        self.disk.release(self.start);
        self.closed = true;
        Ok(())
    }

    pub fn mark_clean(&mut self) -> Result<(), Error> {
        if self.dirty {
            let superblock = self.first_data_block;
            let mut bytes = self.block(superblock)?;
            update_superblock_state(&mut bytes, self.superblock_offset(), true);
            if let Err(error) = self
                .disk
                .write(
                    self.start + u64::from(superblock) * self.block_size as u64,
                    &bytes[..self.block_size],
                )
                .and_then(|_| self.disk.flush())
            {
                self.failure = Some(error);
                return Err(error);
            }
            self.dirty = false;
            self.cache.insert(superblock, bytes);
        }
        Ok(())
    }

    pub fn stage(&mut self, number: u32, bytes: Rc<[u8; MAX_BLOCK_SIZE]>) {
        self.pending.insert(number, bytes);
    }

    pub fn edit(
        &mut self,
        number: u32,
        operation: impl FnOnce(&mut [u8]) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let mut bytes = self.block(number)?;
        operation(&mut Rc::make_mut(&mut bytes)[..])?;
        self.stage(number, bytes);
        Ok(())
    }

    pub fn edit_inode(
        &mut self,
        number: u32,
        operation: impl FnOnce(&mut [u8]) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let group = (number - 1) / self.inodes_per_group;
        let offset = ((number - 1) % self.inodes_per_group) as usize * self.inode_size;
        let table = self.groups[group as usize].inode_table;
        let size = self.inode_size;
        let block_size = self.block_size;
        self.edit(table + (offset / block_size) as u32, |bytes| {
            operation(&mut bytes[offset % block_size..offset % block_size + size])
        })
    }

    pub fn is_open(&self, number: u32) -> bool {
        self.open
            .get(&number)
            .and_then(alloc::rc::Weak::upgrade)
            .is_some()
    }

    pub fn close(&mut self, number: u32) {
        self.open.remove(&number);
        if self.failure.is_some() {
            return;
        }
        let result = self.inode(number).and_then(|mut inode: Inode| {
            if inode.links == 0 {
                self.transaction(|volume| {
                    volume.remove_orphan(&inode)?;
                    volume.destroy_inode(&mut inode)
                })
            } else {
                Ok(())
            }
        });
        if let Err(error) = result {
            self.failure = Some(error);
        }
    }
}

fn update_superblock_state(bytes: &mut Rc<[u8; MAX_BLOCK_SIZE]>, offset: usize, clean: bool) {
    let state = if clean { 1u16 } else { 0u16 };
    Rc::make_mut(bytes)[offset + 58..offset + 60].copy_from_slice(&state.to_le_bytes());
    let incompat = super::word(&bytes[..], offset + 96);
    let incompat = if clean { incompat & !4 } else { incompat | 4 };
    Rc::make_mut(bytes)[offset + 96..offset + 100].copy_from_slice(&incompat.to_le_bytes());
}
