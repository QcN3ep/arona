// SPDX-License-Identifier: MPL-2.0

//! ext4 卷校验、目录查找与 extent 文件访问。

mod extent;
mod inode;
mod volume;

use crate::{Error, FileSystem, ReadAt, block::Disk};
use alloc::{rc::Rc, vec::Vec};
use core::cell::RefCell;
use inode::Inode;
use volume::Volume;

pub struct Ext4 {
    volume: RefCell<Volume>,
}

pub struct File<'a> {
    filesystem: &'a Ext4,
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
            volume: RefCell::new(volume),
        })
    }

    pub fn open_bytes(&self, path: &[u8]) -> Result<File<'_>, Error> {
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
            filesystem: self,
            inode,
        })
    }
}

impl FileSystem for Ext4 {
    type File<'a>
        = File<'a>
    where
        Self: 'a;
    fn open(&self, path: &str) -> Result<Self::File<'_>, Error> {
        self.open_bytes(path.as_bytes())
    }
}

impl ReadAt for File<'_> {
    fn len(&self) -> usize {
        self.inode.size as usize
    }
    fn read_at(&self, offset: usize, output: &mut [u8]) -> Result<usize, Error> {
        self.filesystem
            .volume
            .borrow_mut()
            .read(&self.inode, offset, output)
    }
}

pub(crate) fn components(path: &[u8]) -> Result<Vec<&[u8]>, Error> {
    if path.is_empty() || path.len() > 4096 || path.contains(&0) {
        return Err(Error::InvalidRequest);
    }
    let mut result = Vec::new();
    for part in path.split(|byte| *byte == b'/') {
        if part.is_empty() || part == b"." {
            continue;
        }
        if part == b".." || part.len() > 255 {
            return Err(Error::InvalidRequest);
        }
        result.push(part);
    }
    Ok(result)
}

fn half(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
