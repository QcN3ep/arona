// SPDX-License-Identifier: MPL-2.0

//! ext4 更新暂存与写入失败状态。

use super::{inode::Inode, volume::Volume};
use crate::{Error, block::BLOCK_SIZE};
use alloc::rc::Rc;

impl Volume {
    pub fn transaction<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<T, Error>,
    ) -> Result<T, Error> {
        if self.closed {
            return Err(Error::Unavailable);
        }
        if let Some(error) = self.failure {
            return Err(error);
        }
        if self.disk.read_only() {
            return Err(Error::ReadOnly);
        }
        let value = match operation(self) {
            Ok(value) => value,
            Err(error) => {
                self.pending.clear();
                return Err(error);
            }
        };
        if let Err(error) = self.begin_update() {
            self.pending.clear();
            return Err(error);
        }
        let pending = core::mem::take(&mut self.pending);
        for (number, bytes) in &pending {
            if let Err(error) = self
                .disk
                .write(self.start + u64::from(*number) * BLOCK_SIZE as u64, bytes)
            {
                self.failure = Some(error);
                self.cache.clear();
                return Err(error);
            }
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
            let staged = self.pending.remove(&0);
            let original = self.block(0);
            if let Some(bytes) = staged {
                self.pending.insert(0, bytes);
            }
            let mut bytes = original?;
            Rc::make_mut(&mut bytes)[1082..1084].copy_from_slice(&0u16.to_le_bytes());
            let result = self
                .disk
                .write(self.start, &bytes)
                .and_then(|_| self.disk.flush());
            if let Err(error) = result {
                self.failure = Some(error);
                return Err(error);
            }
            self.cache.insert(0, bytes);
            self.dirty = true;
        }
        if let Some(bytes) = self.pending.get_mut(&0) {
            Rc::make_mut(bytes)[1082..1084].copy_from_slice(&0u16.to_le_bytes());
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
        if self.dirty {
            let mut bytes = self.block(0)?;
            Rc::make_mut(&mut bytes)[1082..1084].copy_from_slice(&1u16.to_le_bytes());
            if let Err(error) = self
                .disk
                .write(self.start, &bytes)
                .and_then(|_| self.disk.flush())
            {
                self.failure = Some(error);
                return Err(error);
            }
            self.dirty = false;
        }
        self.disk.release(self.start);
        self.closed = true;
        Ok(())
    }

    pub fn stage(&mut self, number: u32, bytes: Rc<[u8; BLOCK_SIZE]>) {
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
        self.edit(table + (offset / BLOCK_SIZE) as u32, |bytes| {
            operation(&mut bytes[offset % BLOCK_SIZE..offset % BLOCK_SIZE + size])
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
                self.transaction(|volume| volume.destroy_inode(&mut inode))
            } else {
                Ok(())
            }
        });
        if let Err(error) = result {
            self.failure = Some(error);
        }
    }
}
