// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! FAT32 volume validation, directory lookup, and cluster-chain file reads.

mod directory;
mod volume;

use crate::{Error, FileSystem, ReadAt, block::Disk};
use alloc::{rc::Rc, vec::Vec};
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
        self.volume.borrow_mut().closed = true;
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

impl Node {
    fn metadata(&self, volume: &Volume) -> crate::Metadata {
        let directory = self.entry.directory();
        let size = self.entry.size() as u64;
        crate::Metadata {
            device: volume.device,
            mount: volume.mount,
            inode: self.inode,
            size,
            blocks: size.div_ceil(volume.cluster_size() as u64)
                * (volume.cluster_size() as u64 / 512),
            links: 1,
            mode: if directory {
                0o040755
            } else if self.entry.read_only() {
                0o100444
            } else {
                0o100644
            },
            ..crate::Metadata::default()
        }
    }
}

fn half(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
