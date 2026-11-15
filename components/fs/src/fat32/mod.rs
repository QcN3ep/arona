// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! FAT32 volume access, file reads and writes, and directory operations.

mod directory;
mod edit;
mod metadata;
mod volume;
mod write;

use crate::{Error, FileSystem, ReadAt, block::Disk};
use alloc::{boxed::Box, rc::Rc, vec::Vec};
use core::cell::RefCell;
use directory::Entry;
use volume::Volume;

pub struct Fat32 {
    volume: Rc<RefCell<Volume>>,
}

pub struct File {
    volume: Rc<RefCell<Volume>>,
    node: Rc<RefCell<Node>>,
}

struct Node {
    inode: u64,
    linked: bool,
    entry: Entry,
}

impl Fat32 {
    pub fn mount(disk: Rc<Disk>, start: u64, length: u64) -> Result<Self, Error> {
        Ok(Self {
            volume: Rc::new(RefCell::new(Volume::mount(disk, start, length)?)),
        })
    }

    pub fn busy(&self) -> bool {
        Rc::strong_count(&self.volume) != 1
    }

    pub fn unmount(&self) -> Result<(), Error> {
        if self.busy() {
            return Err(Error::Busy);
        }
        let mut volume = self.volume.borrow_mut();
        volume.sync()?;
        volume.disk.release(volume.start);
        volume.closed = true;
        Ok(())
    }

    pub fn statistics(&self) -> Result<crate::Statistics, Error> {
        self.volume.borrow().statistics()
    }

    pub fn open_bytes(&self, path: &[u8]) -> Result<File, Error> {
        self.open_node(path, false)
    }

    pub fn open_directory(&self, path: &[u8]) -> Result<File, Error> {
        self.open_node(path, true)
    }

    fn open_node(&self, path: &[u8], directory: bool) -> Result<File, Error> {
        let node = self.volume.borrow_mut().lookup(path)?;
        if node.borrow().entry.directory() != directory {
            return Err(if directory {
                Error::NotDirectory
            } else {
                Error::IsDirectory
            });
        }
        Ok(File {
            volume: Rc::clone(&self.volume),
            node,
        })
    }

    pub fn metadata(&self, path: &[u8]) -> Result<crate::Metadata, Error> {
        let mut volume = self.volume.borrow_mut();
        let node = volume.lookup(path)?;
        Ok(node.borrow().metadata(&volume))
    }

    pub fn create(&self, path: &[u8]) -> Result<File, Error> {
        self.create_with_permissions(path, 0o644)
    }

    pub fn create_with_permissions(&self, path: &[u8], permissions: u16) -> Result<File, Error> {
        let node = self.volume.borrow_mut().create(path, false, permissions)?;
        Ok(File {
            volume: Rc::clone(&self.volume),
            node,
        })
    }

    pub fn mkdir(&self, path: &[u8]) -> Result<(), Error> {
        self.mkdir_with_permissions(path, 0o755)
    }

    pub fn mkdir_with_permissions(&self, path: &[u8], permissions: u16) -> Result<(), Error> {
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

    pub fn sync(&self) -> Result<(), Error> {
        self.volume.borrow_mut().sync()
    }

    pub fn open_path(&self, path: &[u8]) -> Result<File, Error> {
        self.open_node(path, false)
    }

    pub fn set_attributes(&self, path: &[u8], attributes: crate::Attributes) -> Result<(), Error> {
        let mut volume = self.volume.borrow_mut();
        let node = volume.lookup(path)?;
        volume.set_attributes(&node, attributes)
    }

    pub fn read_dir(&self, path: &[u8]) -> Result<Vec<crate::DirEntry>, Error> {
        self.open_directory(path)?.read_dir()
    }
}

impl FileSystem for Fat32 {
    type File<'a> = File;
    fn open(&self, path: &str) -> Result<File, Error> {
        self.open_bytes(path.as_bytes())
    }
}

impl ReadAt for File {
    fn len(&self) -> usize {
        self.node.borrow().entry.size()
    }

    fn read_at(&self, offset: usize, output: &mut [u8]) -> Result<usize, Error> {
        let volume = self.volume.borrow();
        let node = self.node.borrow();
        if node.entry.directory() {
            return Err(Error::IsDirectory);
        }
        let length = output.len().min(node.entry.size().saturating_sub(offset));
        if length == 0 {
            return Ok(0);
        }
        let chain = volume.chain(node.entry.first())?;
        let size = volume.cluster_size();
        if chain.len() < node.entry.size().div_ceil(size) {
            return Err(Error::InvalidData);
        }
        let mut done = 0;
        while done < length {
            let position = offset + done;
            let count = (length - done).min(size - position % size);
            volume.read_bytes(
                volume.cluster_offset(chain[position / size])? + (position % size) as u64,
                &mut output[done..done + count],
            )?;
            done += count;
        }
        Ok(done)
    }
}

impl File {
    pub fn path(&self) -> Result<Vec<u8>, Error> {
        self.volume.borrow_mut().directory_path(&self.node)
    }

    pub fn parent_metadata(&self) -> Result<Option<crate::Metadata>, Error> {
        let mut volume = self.volume.borrow_mut();
        let node = self.node.borrow();
        if node.entry.location == volume::ROOT {
            return Ok(None);
        }
        if !node.linked {
            return Err(Error::NotFound);
        }
        Ok(Some(
            volume
                .directory_node(node.entry.parent)?
                .borrow()
                .metadata(&volume),
        ))
    }

    pub fn check_writable(&self) -> Result<(), Error> {
        self.volume
            .borrow()
            .check_file_writable(&self.node.borrow())
    }

    pub fn write_at(&self, offset: usize, input: &[u8]) -> Result<usize, Error> {
        self.volume.borrow_mut().write(&self.node, offset, input)
    }

    pub fn append(&self, input: &[u8]) -> Result<(usize, usize), Error> {
        let offset = self.len();
        let count = self.write_at(offset, input)?;
        Ok((offset, count))
    }

    pub fn truncate(&self, size: usize) -> Result<(), Error> {
        self.volume.borrow_mut().truncate(&self.node, size)
    }

    pub fn fsync(&self) -> Result<(), Error> {
        self.volume.borrow_mut().sync()
    }

    pub fn set_attributes(&self, attributes: crate::Attributes) -> Result<(), Error> {
        self.volume
            .borrow_mut()
            .set_attributes(&self.node, attributes)
    }

    pub fn metadata(&self) -> Result<crate::Metadata, Error> {
        let volume = self.volume.borrow();
        volume.check()?;
        Ok(self.node.borrow().metadata(&volume))
    }

    pub fn statistics(&self) -> Result<crate::Statistics, Error> {
        self.volume.borrow().statistics()
    }

    pub fn read_dir(&self) -> Result<Vec<crate::DirEntry>, Error> {
        let mut volume = self.volume.borrow_mut();
        let node = self.node.borrow();
        if !node.entry.directory() {
            return Err(Error::NotDirectory);
        }
        volume
            .entries(node.entry.first())?
            .into_iter()
            .map(|entry| {
                let name = entry.name.clone();
                let kind = if entry.directory() {
                    crate::FileType::Directory
                } else {
                    crate::FileType::Regular
                };
                let inode = volume.node(entry)?.borrow().inode;
                Ok(crate::DirEntry { name, inode, kind })
            })
            .collect()
    }
}

impl Drop for File {
    fn drop(&mut self) {
        if !self.node.borrow().linked && Rc::strong_count(&self.node) == 1 {
            let first = self.node.borrow().entry.first();
            let mut volume = self.volume.borrow_mut();
            if volume
                .transaction(|volume| volume.free_chain(first))
                .is_err()
            {
                volume.failed = true;
            }
        }
    }
}

fn half(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

impl crate::backend::Backend for Fat32 {
    fn sync(&self) -> Result<(), Error> {
        Fat32::sync(self)
    }
    fn open_path(&self, path: &[u8]) -> Result<Box<dyn crate::backend::File>, Error> {
        Fat32::open_path(self, path).map(|file| Box::new(file) as Box<dyn crate::backend::File>)
    }

    fn statistics(&self) -> Result<crate::Statistics, Error> {
        Fat32::statistics(self)
    }

    fn unmount(&self) -> Result<(), Error> {
        Fat32::unmount(self)
    }

    fn open_bytes(&self, path: &[u8]) -> Result<Box<dyn crate::backend::File>, Error> {
        Fat32::open_bytes(self, path).map(|file| Box::new(file) as Box<dyn crate::backend::File>)
    }

    fn open_directory(&self, path: &[u8]) -> Result<Box<dyn crate::backend::File>, Error> {
        Fat32::open_directory(self, path)
            .map(|file| Box::new(file) as Box<dyn crate::backend::File>)
    }

    fn create(&self, path: &[u8]) -> Result<Box<dyn crate::backend::File>, Error> {
        Fat32::create(self, path).map(|file| Box::new(file) as Box<dyn crate::backend::File>)
    }

    fn create_with_permissions(
        &self,
        path: &[u8],
        permissions: u16,
    ) -> Result<Box<dyn crate::backend::File>, Error> {
        Fat32::create_with_permissions(self, path, permissions)
            .map(|file| Box::new(file) as Box<dyn crate::backend::File>)
    }

    fn metadata(&self, path: &[u8]) -> Result<crate::Metadata, Error> {
        Fat32::metadata(self, path)
    }

    fn mkdir_with_permissions(&self, path: &[u8], permissions: u16) -> Result<(), Error> {
        Fat32::mkdir_with_permissions(self, path, permissions)
    }

    fn unlink(&self, path: &[u8]) -> Result<(), Error> {
        Fat32::unlink(self, path)
    }

    fn rename(&self, source: &[u8], target: &[u8]) -> Result<(), Error> {
        Fat32::rename(self, source, target)
    }

    fn read_dir(&self, path: &[u8]) -> Result<Vec<crate::DirEntry>, Error> {
        Fat32::read_dir(self, path)
    }
}

impl crate::backend::File for File {
    fn statistics(&self) -> Result<crate::Statistics, Error> {
        File::statistics(self)
    }

    fn path(&self) -> Result<Vec<u8>, Error> {
        File::path(self)
    }

    fn check_writable(&self) -> Result<(), Error> {
        File::check_writable(self)
    }

    fn read_dir(&self) -> Result<Vec<crate::DirEntry>, Error> {
        File::read_dir(self)
    }

    fn metadata(&self) -> Result<crate::Metadata, Error> {
        File::metadata(self)
    }

    fn parent_metadata(&self) -> Result<Option<crate::Metadata>, Error> {
        File::parent_metadata(self)
    }

    fn set_attributes(&self, attributes: crate::Attributes) -> Result<(), Error> {
        File::set_attributes(self, attributes)
    }

    fn append(&self, bytes: &[u8]) -> Result<(usize, usize), Error> {
        File::append(self, bytes)
    }

    fn fsync(&self) -> Result<(), Error> {
        File::fsync(self)
    }

    fn write_at(&self, offset: usize, bytes: &[u8]) -> Result<usize, Error> {
        File::write_at(self, offset, bytes)
    }

    fn truncate(&self, size: usize) -> Result<(), Error> {
        File::truncate(self, size)
    }
}
