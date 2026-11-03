// SPDX-License-Identifier: MPL-2.0

//! ext4 卷校验、目录查找与 extent 文件访问。

mod extent;
mod inode;
mod volume;

use crate::path::components;
use crate::{Error, FileSystem, ReadAt, block::Disk};
use alloc::rc::Rc;
use core::cell::RefCell;
use inode::Inode;
use volume::Volume;

pub struct Ext4 {
    volume: Rc<RefCell<Volume>>,
}

pub struct File {
    volume: Rc<RefCell<Volume>>,
    inode: Inode,
}

impl Ext4 {
    pub fn mount(disk: Rc<Disk>, start: u64, length: u64) -> Result<Self, Error> {
        let mut volume = Volume::mount(disk, start, length)?;
        let root = volume.inode(2)?;
        if !root.directory() {
            return Err(Error::InvalidData);
        }
        volume.entries(&root)?;
        Ok(Self {
            volume: Rc::new(RefCell::new(volume)),
        })
    }

    pub fn busy(&self) -> bool {
        Rc::strong_count(&self.volume) != 1
    }

    pub fn open_bytes(&self, path: &[u8]) -> Result<File, Error> {
        let components = components(path)?;
        let mut volume = self.volume.borrow_mut();
        let mut inode = volume.inode(2)?;
        for name in components {
            if !inode.directory() {
                return Err(Error::NotDirectory);
            }
            let entries = volume.entries(&inode)?;
            let entry = entries
                .iter()
                .find(|entry| entry.name == name)
                .ok_or(Error::NotFound)?;
            inode = volume.inode(entry.inode)?;
        }
        if inode.directory() {
            return Err(Error::IsDirectory);
        }
        Ok(File {
            volume: Rc::clone(&self.volume),
            inode,
        })
    }
}

impl FileSystem for Ext4 {
    type File<'a>
        = File
    where
        Self: 'a;
    fn open(&self, path: &str) -> Result<Self::File<'_>, Error> {
        self.open_bytes(path.as_bytes())
    }
}

impl ReadAt for File {
    fn len(&self) -> usize {
        self.inode.size as usize
    }
    fn read_at(&self, offset: usize, output: &mut [u8]) -> Result<usize, Error> {
        self.volume.borrow_mut().read(&self.inode, offset, output)
    }
}

fn half(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
