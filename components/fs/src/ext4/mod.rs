// SPDX-License-Identifier: MPL-2.0

//! ext4 卷校验、目录查找与 extent 文件访问。

mod allocator;
mod directory;
mod extent;
mod inode;
mod transaction;
mod volume;
mod write;

use crate::path::components;
use crate::{Error, FileSystem, ReadAt, block::Disk};
use alloc::{rc::Rc, vec::Vec};
use core::cell::{Cell, RefCell};
use volume::Volume;

pub struct Ext4 {
    volume: Rc<RefCell<Volume>>,
}

pub struct File {
    volume: Rc<RefCell<Volume>>,
    identity: Rc<FileIdentity>,
}

pub(super) struct FileIdentity {
    number: u32,
    size: Cell<usize>,
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
        let mut volume = self.volume.borrow_mut();
        let inode = volume.lookup(path)?;
        if inode.directory() {
            return Err(Error::IsDirectory);
        }
        let identity = volume
            .open
            .get(&inode.number)
            .and_then(alloc::rc::Weak::upgrade)
            .unwrap_or_else(|| {
                let identity = Rc::new(FileIdentity {
                    number: inode.number,
                    size: Cell::new(inode.size as usize),
                });
                volume.open.insert(inode.number, Rc::downgrade(&identity));
                identity
            });
        Ok(File {
            volume: Rc::clone(&self.volume),
            identity,
        })
    }

    pub fn create(&self, path: &[u8]) -> Result<File, Error> {
        self.volume.borrow_mut().create(path, false)?;
        self.open_bytes(path)
    }
    pub fn mkdir(&self, path: &[u8]) -> Result<(), Error> {
        self.volume.borrow_mut().create(path, true).map(|_| ())
    }
    pub fn unlink(&self, path: &[u8]) -> Result<(), Error> {
        self.volume.borrow_mut().unlink(path)
    }
    pub fn rename(&self, source: &[u8], target: &[u8]) -> Result<(), Error> {
        self.volume.borrow_mut().rename(source, target)
    }
    pub fn read_dir(&self, path: &[u8]) -> Result<Vec<crate::DirEntry>, Error> {
        let mut volume = self.volume.borrow_mut();
        let inode = volume.lookup(path)?;
        if !inode.directory() {
            return Err(Error::NotDirectory);
        }
        Ok(volume
            .entries(&inode)?
            .into_iter()
            .filter(|entry| entry.name != b"." && entry.name != b"..")
            .map(|entry| crate::DirEntry {
                name: entry.name,
                directory: entry.file_type == 2,
            })
            .collect())
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
        self.identity.size.get()
    }
    fn read_at(&self, offset: usize, output: &mut [u8]) -> Result<usize, Error> {
        let mut volume = self.volume.borrow_mut();
        let inode = volume.inode(self.identity.number)?;
        volume.read(&inode, offset, output)
    }
}

impl File {
    pub fn write_at(&self, offset: usize, input: &[u8]) -> Result<usize, Error> {
        let count = self
            .volume
            .borrow_mut()
            .write(self.identity.number, offset, input)?;
        if count != 0 {
            self.identity.size.set(self.len().max(offset + count));
        }
        Ok(count)
    }
    pub fn truncate(&self, size: usize) -> Result<(), Error> {
        self.volume
            .borrow_mut()
            .truncate(self.identity.number, size)?;
        self.identity.size.set(size);
        Ok(())
    }
}

impl Drop for File {
    fn drop(&mut self) {
        if Rc::strong_count(&self.identity) == 1 {
            self.volume.borrow_mut().close(self.identity.number);
        }
    }
}

fn half(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
