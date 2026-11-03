// SPDX-License-Identifier: MPL-2.0

//! 文件命名空间与挂载所有权。

use crate::{
    Error, FileSystem, ReadAt,
    archive::{self, Archive},
    block::Disk,
    ext4::Ext4,
    path::normalize,
};
use alloc::{collections::BTreeMap, rc::Rc, vec::Vec};
use core::cell::RefCell;

pub struct Namespace {
    archive: Archive,
    mounts: RefCell<BTreeMap<Vec<u8>, Ext4>>,
}

pub enum File {
    Archive(archive::File),
    Ext4(crate::ext4::File),
}

impl Namespace {
    pub fn new(archive: Archive) -> Self {
        Self {
            archive,
            mounts: RefCell::new(BTreeMap::new()),
        }
    }

    pub fn mount(&self, path: &[u8], disk: Rc<Disk>, start: u64, length: u64) -> Result<(), Error> {
        let path = normalize(path)?;
        let mut mounts = self.mounts.borrow_mut();
        if path.is_empty()
            || mounts
                .keys()
                .any(|other| beneath(&path, other) || beneath(other, &path))
        {
            return Err(Error::Busy);
        }
        let filesystem = Ext4::mount(disk, start, length)?;
        mounts.insert(path, filesystem);
        Ok(())
    }

    pub fn unmount(&self, path: &[u8]) -> Result<(), Error> {
        let path = normalize(path)?;
        let mut mounts = self.mounts.borrow_mut();
        let filesystem = mounts.get(&path).ok_or(Error::NotFound)?;
        if filesystem.busy() {
            return Err(Error::Busy);
        }
        mounts.remove(&path);
        Ok(())
    }

    pub fn open_bytes(&self, path: &[u8]) -> Result<File, Error> {
        let path = normalize(path)?;
        for (prefix, filesystem) in self.mounts.borrow().iter() {
            if beneath(&path, prefix) {
                let relative = path.get(prefix.len() + 1..).unwrap_or(b"/");
                return filesystem.open_bytes(relative).map(File::Ext4);
            }
        }
        self.archive.open_bytes(&path).map(File::Archive)
    }
}

fn beneath(path: &[u8], prefix: &[u8]) -> bool {
    path == prefix || path.starts_with(prefix) && path.get(prefix.len()) == Some(&b'/')
}

impl FileSystem for Namespace {
    type File<'a> = File;
    fn open(&self, path: &str) -> Result<File, Error> {
        self.open_bytes(path.as_bytes())
    }
}

impl ReadAt for File {
    fn len(&self) -> usize {
        match self {
            Self::Archive(file) => file.len(),
            Self::Ext4(file) => file.len(),
        }
    }
    fn read_at(&self, offset: usize, output: &mut [u8]) -> Result<usize, Error> {
        match self {
            Self::Archive(file) => file.read_at(offset, output),
            Self::Ext4(file) => file.read_at(offset, output),
        }
    }
}
