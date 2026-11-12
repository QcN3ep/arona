// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! ext4 volume validation, directory lookup, and extent file access.

mod allocator;
mod directory;
mod extent;
mod inode;
mod journal;
mod metadata;
mod recovery;
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
        let volume = Volume::mount(disk, start, length)?;
        Ok(Self {
            volume: Rc::new(RefCell::new(volume)),
        })
    }

    pub fn unmount(&self) -> Result<(), Error> {
        if self.busy() {
            return Err(Error::Busy);
        }
        self.volume.borrow_mut().unmount()
    }

    pub fn busy(&self) -> bool {
        Rc::strong_count(&self.volume) != 1
    }

    pub fn open_bytes(&self, path: &[u8]) -> Result<File, Error> {
        self.open_node(path, false)
    }

    pub fn open_directory(&self, path: &[u8]) -> Result<File, Error> {
        self.open_node(path, true)
    }

    fn open_node(&self, path: &[u8], directory: bool) -> Result<File, Error> {
        let mut volume = self.volume.borrow_mut();
        let inode = volume.lookup(path)?;
        if inode.directory() != directory {
            return Err(if directory {
                Error::NotDirectory
            } else {
                Error::IsDirectory
            });
        }
        if inode.symlink() {
            return Err(Error::InvalidRequest);
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
        self.create_with_permissions(path, 0o644)
    }

    pub fn create_with_permissions(&self, path: &[u8], permissions: u16) -> Result<File, Error> {
        if permissions & !0o7777 != 0 {
            return Err(Error::InvalidRequest);
        }
        self.volume.borrow_mut().create(path, false, permissions)?;
        self.open_bytes(path)
    }
    pub fn metadata(&self, path: &[u8]) -> Result<crate::Metadata, Error> {
        let mut volume = self.volume.borrow_mut();
        Ok(volume.lookup(path)?.metadata(volume.device, volume.mount))
    }
    pub fn hard_link(&self, source: &[u8], target: &[u8]) -> Result<(), Error> {
        self.volume.borrow_mut().hard_link(source, target)
    }
    pub fn symlink(&self, target: &[u8], path: &[u8]) -> Result<(), Error> {
        self.volume.borrow_mut().symlink(target, path)
    }
    pub fn read_link(&self, path: &[u8]) -> Result<Vec<u8>, Error> {
        self.volume.borrow_mut().read_link(path)
    }
    pub fn mkdir(&self, path: &[u8]) -> Result<(), Error> {
        self.mkdir_with_permissions(path, 0o755)
    }
    pub fn mkdir_with_permissions(&self, path: &[u8], permissions: u16) -> Result<(), Error> {
        if permissions & !0o7777 != 0 {
            return Err(Error::InvalidRequest);
        }
        self.volume
            .borrow_mut()
            .create(path, true, permissions)
            .map(|_| ())
    }
    pub fn unlink(&self, path: &[u8]) -> Result<(), Error> {
        self.volume.borrow_mut().unlink(path)
    }
    pub fn rename(&self, source: &[u8], target: &[u8]) -> Result<(), Error> {
        self.volume.borrow_mut().rename(source, target)
    }
    pub fn read_dir(&self, path: &[u8]) -> Result<Vec<crate::DirEntry>, Error> {
        self.open_directory(path)?.read_dir()
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
        if inode.directory() {
            return Err(Error::IsDirectory);
        }
        volume.read(&inode, offset, output)
    }
}

impl File {
    pub fn path(&self) -> Result<Vec<u8>, Error> {
        self.volume
            .borrow_mut()
            .directory_path(self.identity.number)
    }

    pub fn check_writable(&self) -> Result<(), Error> {
        self.volume.borrow().check_writable()
    }

    pub fn read_dir(&self) -> Result<Vec<crate::DirEntry>, Error> {
        let mut volume = self.volume.borrow_mut();
        let inode = volume.inode(self.identity.number)?;
        if !inode.directory() {
            return Err(Error::NotDirectory);
        }
        Ok(volume
            .entries(&inode)?
            .into_iter()
            .filter(|entry| entry.name != b"." && entry.name != b"..")
            .map(|entry| crate::DirEntry {
                name: entry.name,
                inode: u64::from(entry.inode),
                kind: match entry.file_type {
                    1 => crate::FileType::Regular,
                    2 => crate::FileType::Directory,
                    7 => crate::FileType::SymbolicLink,
                    _ => crate::FileType::Unknown,
                },
            })
            .collect())
    }
    pub fn metadata(&self) -> Result<crate::Metadata, Error> {
        let mut volume = self.volume.borrow_mut();
        Ok(volume
            .inode(self.identity.number)?
            .metadata(volume.device, volume.mount))
    }
    pub fn parent_metadata(&self) -> Result<Option<crate::Metadata>, Error> {
        if self.identity.number == 2 {
            return Ok(None);
        }
        let mut volume = self.volume.borrow_mut();
        let inode = volume.inode(self.identity.number)?;
        let parent = volume
            .entries(&inode)?
            .into_iter()
            .find(|entry| entry.name == b"..")
            .ok_or(Error::InvalidData)?
            .inode;
        Ok(Some(
            volume.inode(parent)?.metadata(volume.device, volume.mount),
        ))
    }
    pub fn set_attributes(&self, attributes: crate::Attributes) -> Result<(), Error> {
        self.volume
            .borrow_mut()
            .set_attributes(self.identity.number, attributes)
    }
    pub fn append(&self, input: &[u8]) -> Result<(usize, usize), Error> {
        let mut volume = self.volume.borrow_mut();
        let offset = volume.inode(self.identity.number)?.size as usize;
        let count = volume.write(self.identity.number, offset, input)?;
        self.identity.size.set(offset + count);
        Ok((offset, count))
    }
    pub fn fsync(&self) -> Result<(), Error> {
        self.volume.borrow_mut().sync()
    }

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
