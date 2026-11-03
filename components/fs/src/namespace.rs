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
        filesystem.unmount()?;
        mounts.remove(&path);
        Ok(())
    }

    fn mounted<T>(
        &self,
        path: &[u8],
        operation: impl FnOnce(&Ext4, &[u8]) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let path = crate::path::normalize(path)?;
        for (prefix, filesystem) in self.mounts.borrow().iter() {
            if beneath(&path, prefix) {
                return operation(filesystem, path.get(prefix.len() + 1..).unwrap_or(b"/"));
            }
        }
        Err(Error::ReadOnly)
    }

    pub fn create(&self, path: &[u8]) -> Result<File, Error> {
        self.mounted(path, |fs, path| fs.create(path).map(File::Ext4))
    }
    pub fn mkdir(&self, path: &[u8]) -> Result<(), Error> {
        self.mounted(path, |fs, path| fs.mkdir(path))
    }
    pub fn unlink(&self, path: &[u8]) -> Result<(), Error> {
        self.mounted(path, |fs, path| fs.unlink(path))
    }
    pub fn read_dir(&self, path: &[u8]) -> Result<Vec<crate::DirEntry>, Error> {
        let path = crate::path::normalize(path)?;
        let mounts = self.mounts.borrow();
        for (prefix, fs) in mounts.iter() {
            if beneath(&path, prefix) {
                return fs.read_dir(path.get(prefix.len() + 1..).unwrap_or(b"/"));
            }
        }
        let mut entries = self.archive.read_dir(&path)?;
        for prefix in mounts.keys() {
            let parent = prefix
                .iter()
                .rposition(|byte| *byte == b'/')
                .map_or(b"".as_slice(), |end| &prefix[..end]);
            if parent == path {
                let name = prefix
                    .get(
                        if parent.is_empty() {
                            0
                        } else {
                            parent.len() + 1
                        }..,
                    )
                    .unwrap();
                entries.retain(|entry| entry.name != name);
                entries.push(crate::DirEntry {
                    name: name.to_vec(),
                    directory: true,
                });
            }
        }
        entries.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }
    pub fn rename(&self, source: &[u8], target: &[u8]) -> Result<(), Error> {
        let source = crate::path::normalize(source)?;
        let target = crate::path::normalize(target)?;
        for (prefix, filesystem) in self.mounts.borrow().iter() {
            if beneath(&source, prefix) {
                if !beneath(&target, prefix) {
                    return Err(Error::CrossDevice);
                }
                return filesystem.rename(
                    source.get(prefix.len() + 1..).unwrap_or(b"/"),
                    target.get(prefix.len() + 1..).unwrap_or(b"/"),
                );
            }
        }
        Err(Error::ReadOnly)
    }

    pub fn shutdown(&self) -> Result<(), Error> {
        let mut mounts = self.mounts.borrow_mut();
        while let Some(entry) = mounts.first_entry() {
            entry.get().unmount()?;
            entry.remove();
        }
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

impl File {
    pub fn fsync(&self) -> Result<(), Error> {
        match self {
            Self::Archive(_) => Ok(()),
            Self::Ext4(file) => file.fsync(),
        }
    }
    pub fn write_at(&self, offset: usize, bytes: &[u8]) -> Result<usize, Error> {
        match self {
            Self::Archive(_) => Err(Error::ReadOnly),
            Self::Ext4(file) => file.write_at(offset, bytes),
        }
    }
    pub fn truncate(&self, size: usize) -> Result<(), Error> {
        match self {
            Self::Archive(_) => Err(Error::ReadOnly),
            Self::Ext4(file) => file.truncate(size),
        }
    }
}
