// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! File namespace and mount ownership.

use crate::{
    Error, FileSystem, ReadAt,
    archive::{self, Archive},
    block::Disk,
    ext4::Ext4,
    path::normalize,
    tmpfs::Tmpfs,
};
use alloc::{collections::BTreeMap, rc::Rc, vec::Vec};
use core::cell::{Cell, RefCell};

pub struct Namespace {
    archive: Archive,
    next_directory: Cell<u64>,
    mounts: RefCell<BTreeMap<Vec<u8>, Mounted>>,
}

/// A filesystem mounted at a path in the namespace.
enum Mounted {
    Ext4(Ext4),
    Tmpfs(Tmpfs),
}

impl Mounted {
    fn statistics(&self) -> Result<crate::Statistics, Error> {
        match self {
            Self::Ext4(filesystem) => filesystem.statistics(),
            Self::Tmpfs(filesystem) => filesystem.statistics(),
        }
    }

    fn unmount(&self) -> Result<(), Error> {
        match self {
            Self::Ext4(filesystem) => filesystem.unmount(),
            Self::Tmpfs(filesystem) => filesystem.unmount(),
        }
    }

    fn metadata(&self, path: &[u8]) -> Result<crate::Metadata, Error> {
        match self {
            Self::Ext4(filesystem) => filesystem.metadata(path),
            Self::Tmpfs(filesystem) => filesystem.metadata(path),
        }
    }

    fn read_dir(&self, path: &[u8]) -> Result<Vec<crate::DirEntry>, Error> {
        match self {
            Self::Ext4(filesystem) => filesystem.read_dir(path),
            Self::Tmpfs(filesystem) => filesystem.read_dir(path),
        }
    }

    fn rename(&self, source: &[u8], target: &[u8]) -> Result<(), Error> {
        match self {
            Self::Ext4(filesystem) => filesystem.rename(source, target),
            Self::Tmpfs(filesystem) => filesystem.rename(source, target),
        }
    }

    fn hard_link(&self, source: &[u8], target: &[u8]) -> Result<(), Error> {
        match self {
            Self::Ext4(filesystem) => filesystem.hard_link(source, target),
            Self::Tmpfs(filesystem) => filesystem.hard_link(source, target),
        }
    }

    fn symlink(&self, target: &[u8], path: &[u8]) -> Result<(), Error> {
        match self {
            Self::Ext4(filesystem) => filesystem.symlink(target, path),
            Self::Tmpfs(filesystem) => filesystem.symlink(target, path),
        }
    }

    fn read_link(&self, path: &[u8]) -> Result<Vec<u8>, Error> {
        match self {
            Self::Ext4(filesystem) => filesystem.read_link(path),
            Self::Tmpfs(filesystem) => filesystem.read_link(path),
        }
    }

    fn open_bytes(&self, path: &[u8]) -> Result<File, Error> {
        match self {
            Self::Ext4(filesystem) => filesystem.open_bytes(path).map(File::Ext4),
            Self::Tmpfs(filesystem) => filesystem.open_bytes(path).map(File::Tmpfs),
        }
    }

    fn open_directory(&self, path: &[u8]) -> Result<DirectoryNode, Error> {
        match self {
            Self::Ext4(filesystem) => filesystem.open_directory(path).map(DirectoryNode::Ext4),
            Self::Tmpfs(filesystem) => filesystem.open_directory(path).map(DirectoryNode::Tmpfs),
        }
    }

    fn create(&self, path: &[u8]) -> Result<File, Error> {
        match self {
            Self::Ext4(filesystem) => filesystem.create(path).map(File::Ext4),
            Self::Tmpfs(filesystem) => filesystem.create(path).map(File::Tmpfs),
        }
    }

    fn create_with_permissions(&self, path: &[u8], permissions: u16) -> Result<File, Error> {
        match self {
            Self::Ext4(filesystem) => filesystem
                .create_with_permissions(path, permissions)
                .map(File::Ext4),
            Self::Tmpfs(filesystem) => filesystem
                .create_with_permissions(path, permissions)
                .map(File::Tmpfs),
        }
    }

    fn mkdir_with_permissions(&self, path: &[u8], permissions: u16) -> Result<(), Error> {
        match self {
            Self::Ext4(filesystem) => filesystem.mkdir_with_permissions(path, permissions),
            Self::Tmpfs(filesystem) => filesystem.mkdir_with_permissions(path, permissions),
        }
    }

    fn unlink(&self, path: &[u8]) -> Result<(), Error> {
        match self {
            Self::Ext4(filesystem) => filesystem.unlink(path),
            Self::Tmpfs(filesystem) => filesystem.unlink(path),
        }
    }
}

pub enum File {
    Archive(archive::File),
    Ext4(crate::ext4::File),
    Tmpfs(crate::tmpfs::File),
}

pub struct Directory {
    pub(crate) id: u64,
    node: DirectoryNode,
    parent: crate::Metadata,
    path: Vec<u8>,
}

enum DirectoryNode {
    Archive(archive::Directory),
    Ext4(crate::ext4::File),
    Tmpfs(crate::tmpfs::File),
}

impl Directory {
    pub fn statistics(&self) -> Result<crate::Statistics, Error> {
        match &self.node {
            DirectoryNode::Archive(directory) => directory.statistics(),
            DirectoryNode::Ext4(directory) => directory.statistics(),
            DirectoryNode::Tmpfs(directory) => directory.statistics(),
        }
    }

    pub fn path(&self) -> Result<Vec<u8>, Error> {
        let mut path = self.path.clone();
        let relative = match &self.node {
            DirectoryNode::Archive(_) => Vec::new(),
            DirectoryNode::Ext4(directory) => directory.path()?,
            DirectoryNode::Tmpfs(directory) => directory.path()?,
        };
        if !relative.is_empty() {
            path.push(b'/');
            path.extend(relative);
        }
        path.insert(0, b'/');
        Ok(path)
    }

    pub fn fsync(&self) -> Result<(), Error> {
        match &self.node {
            DirectoryNode::Archive(_) => Ok(()),
            DirectoryNode::Ext4(directory) => directory.fsync(),
            DirectoryNode::Tmpfs(directory) => directory.fsync(),
        }
    }

    pub fn metadata(&self) -> Result<crate::Metadata, Error> {
        match &self.node {
            DirectoryNode::Archive(directory) => Ok(directory.metadata()),
            DirectoryNode::Ext4(directory) => directory.metadata(),
            DirectoryNode::Tmpfs(directory) => directory.metadata(),
        }
    }
    pub fn read_dir(&self) -> Result<Vec<crate::DirEntry>, Error> {
        match &self.node {
            DirectoryNode::Archive(directory) => Ok(directory.read_dir()),
            DirectoryNode::Ext4(directory) => directory.read_dir(),
            DirectoryNode::Tmpfs(directory) => directory.read_dir(),
        }
    }
    pub fn read_dir_all(&self) -> Result<Vec<crate::DirEntry>, Error> {
        let parent = match &self.node {
            DirectoryNode::Ext4(directory) => directory.parent_metadata()?.unwrap_or(self.parent),
            DirectoryNode::Tmpfs(directory) => directory.parent_metadata()?.unwrap_or(self.parent),
            DirectoryNode::Archive(_) => self.parent,
        };
        let mut entries = alloc::vec![
            crate::DirEntry {
                name: b".".to_vec(),
                inode: self.metadata()?.inode,
                kind: crate::FileType::Directory
            },
            crate::DirEntry {
                name: b"..".to_vec(),
                inode: parent.inode,
                kind: crate::FileType::Directory
            },
        ];
        entries.extend(self.read_dir()?);
        Ok(entries)
    }
    pub fn set_attributes(&self, attributes: crate::Attributes) -> Result<(), Error> {
        match &self.node {
            DirectoryNode::Archive(_) => Err(Error::ReadOnly),
            DirectoryNode::Ext4(directory) => directory.set_attributes(attributes),
            DirectoryNode::Tmpfs(directory) => directory.set_attributes(attributes),
        }
    }
}

impl Namespace {
    pub fn statistics(&self, path: &[u8]) -> Result<crate::Statistics, Error> {
        let path = normalize(&self.resolve(b"/", path, true, false)?)?;
        for (prefix, filesystem) in self.mounts.borrow().iter() {
            if beneath(&path, prefix) {
                return filesystem.statistics();
            }
        }
        self.archive.statistics()
    }

    pub fn new(archive: Archive) -> Self {
        Self {
            archive,
            next_directory: Cell::new(1),
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
        mounts.insert(path, Mounted::Ext4(filesystem));
        Ok(())
    }

    /// Mounts a fresh in-memory filesystem at a path.
    pub fn mount_tmpfs(&self, path: &[u8], capacity: usize) -> Result<(), Error> {
        let path = normalize(path)?;
        let mut mounts = self.mounts.borrow_mut();
        if path.is_empty()
            || mounts
                .keys()
                .any(|other| beneath(&path, other) || beneath(other, &path))
        {
            return Err(Error::Busy);
        }
        let filesystem = if capacity == 0 {
            Tmpfs::new()?
        } else {
            Tmpfs::with_capacity(capacity)?
        };
        mounts.insert(path, Mounted::Tmpfs(filesystem));
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
        operation: impl FnOnce(&Mounted, &[u8]) -> Result<T, Error>,
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
        self.mounted(path, Mounted::create)
    }

    pub fn open_with(&self, path: &[u8], options: crate::OpenOptions) -> Result<File, Error> {
        options.validate()?;
        let file = match self.open_bytes(path) {
            Ok(_) if options.create && options.exclusive => return Err(Error::AlreadyExists),
            Err(Error::IsDirectory) if options.create && options.exclusive => {
                return Err(Error::AlreadyExists);
            }
            Ok(file) => file,
            Err(Error::NotFound) if options.create => self.mounted(path, |fs, path| {
                fs.create_with_permissions(path, options.permissions)
            })?,
            Err(error) => return Err(error),
        };
        if options.write || options.truncate {
            file.check_writable()?;
        }
        if options.truncate {
            file.truncate(0)?;
        }
        Ok(file)
    }
    pub fn mkdir(&self, path: &[u8]) -> Result<(), Error> {
        self.mkdir_with_permissions(path, 0o755)
    }
    pub fn mkdir_with_permissions(&self, path: &[u8], permissions: u16) -> Result<(), Error> {
        self.mounted(path, |fs, path| {
            fs.mkdir_with_permissions(path, permissions)
        })
    }
    pub fn unlink(&self, path: &[u8]) -> Result<(), Error> {
        let normalized = normalize(path)?;
        if normalized.is_empty() || self.mounts.borrow().contains_key(&normalized) {
            return Err(Error::Busy);
        }
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
        let mut entries = self
            .archive
            .read_dir(if path.is_empty() { b"/" } else { &path })?;
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
                    inode: mounts.get(prefix).unwrap().metadata(b"/")?.inode,
                    kind: crate::FileType::Directory,
                });
            }
        }
        entries.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }
    pub fn rename(&self, source: &[u8], target: &[u8]) -> Result<(), Error> {
        let source = crate::path::normalize(source)?;
        let target = crate::path::normalize(target)?;
        if source.is_empty()
            || target.is_empty()
            || self.mounts.borrow().contains_key(&source)
            || self.mounts.borrow().contains_key(&target)
        {
            return Err(Error::Busy);
        }
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

    pub fn metadata(&self, path: &[u8]) -> Result<crate::Metadata, Error> {
        let path = normalize(path)?;
        for (prefix, fs) in self.mounts.borrow().iter() {
            if beneath(&path, prefix) {
                return fs.metadata(path.get(prefix.len() + 1..).unwrap_or(b"/"));
            }
        }
        self.archive
            .metadata(if path.is_empty() { b"/" } else { &path })
    }
    pub fn open_directory(&self, path: &[u8]) -> Result<Directory, Error> {
        let id = self.next_directory.get();
        self.next_directory
            .set(id.checked_add(1).ok_or(Error::NoSpace)?);
        let path = normalize(path)?;
        let parent_path = path
            .iter()
            .rposition(|byte| *byte == b'/')
            .map_or(b"/".as_slice(), |end| &path[..end]);
        let parent = self.metadata(parent_path)?;
        for (prefix, fs) in self.mounts.borrow().iter() {
            if beneath(&path, prefix) {
                let node = fs.open_directory(path.get(prefix.len() + 1..).unwrap_or(b"/"))?;
                return Ok(Directory {
                    id,
                    node,
                    parent,
                    path: prefix.clone(),
                });
            }
        }
        let mut directory =
            self.archive
                .open_directory(if path.is_empty() { b"/" } else { &path })?;
        directory.set_entries(self.read_dir(if path.is_empty() { b"/" } else { &path })?);
        Ok(Directory {
            id,
            node: DirectoryNode::Archive(directory),
            parent,
            path,
        })
    }
    pub fn hard_link(&self, source: &[u8], target: &[u8]) -> Result<(), Error> {
        let source = normalize(source)?;
        let target = normalize(target)?;
        for (prefix, fs) in self.mounts.borrow().iter() {
            if beneath(&source, prefix) {
                if !beneath(&target, prefix) {
                    return Err(Error::CrossDevice);
                }
                return fs.hard_link(
                    source.get(prefix.len() + 1..).unwrap_or(b"/"),
                    target.get(prefix.len() + 1..).unwrap_or(b"/"),
                );
            }
        }
        Err(Error::ReadOnly)
    }
    pub fn symlink(&self, target: &[u8], path: &[u8]) -> Result<(), Error> {
        self.mounted(path, |fs, path| fs.symlink(target, path))
    }
    pub fn read_link(&self, path: &[u8]) -> Result<Vec<u8>, Error> {
        let path = normalize(path)?;
        for (prefix, fs) in self.mounts.borrow().iter() {
            if beneath(&path, prefix) {
                return fs.read_link(path.get(prefix.len() + 1..).unwrap_or(b"/"));
            }
        }
        self.archive.read_link(&path)
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
                return filesystem.open_bytes(relative);
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
            Self::Tmpfs(file) => file.len(),
        }
    }
    fn read_at(&self, offset: usize, output: &mut [u8]) -> Result<usize, Error> {
        match self {
            Self::Archive(file) => file.read_at(offset, output),
            Self::Ext4(file) => file.read_at(offset, output),
            Self::Tmpfs(file) => file.read_at(offset, output),
        }
    }
}

impl File {
    pub fn statistics(&self) -> Result<crate::Statistics, Error> {
        match self {
            Self::Archive(file) => file.statistics(),
            Self::Ext4(file) => file.statistics(),
            Self::Tmpfs(file) => file.statistics(),
        }
    }

    pub fn check_writable(&self) -> Result<(), Error> {
        match self {
            Self::Archive(_) => Err(Error::ReadOnly),
            Self::Ext4(file) => file.check_writable(),
            Self::Tmpfs(file) => file.check_writable(),
        }
    }

    pub fn metadata(&self) -> Result<crate::Metadata, Error> {
        match self {
            Self::Archive(file) => Ok(file.metadata()),
            Self::Ext4(file) => file.metadata(),
            Self::Tmpfs(file) => file.metadata(),
        }
    }
    pub fn set_attributes(&self, attributes: crate::Attributes) -> Result<(), Error> {
        match self {
            Self::Archive(_) => Err(Error::ReadOnly),
            Self::Ext4(file) => file.set_attributes(attributes),
            Self::Tmpfs(file) => file.set_attributes(attributes),
        }
    }
    pub fn append(&self, bytes: &[u8]) -> Result<(usize, usize), Error> {
        match self {
            Self::Archive(_) => Err(Error::ReadOnly),
            Self::Ext4(file) => file.append(bytes),
            Self::Tmpfs(file) => file.append(bytes),
        }
    }

    pub fn fsync(&self) -> Result<(), Error> {
        match self {
            Self::Archive(_) => Ok(()),
            Self::Ext4(file) => file.fsync(),
            Self::Tmpfs(file) => file.fsync(),
        }
    }
    pub fn write_at(&self, offset: usize, bytes: &[u8]) -> Result<usize, Error> {
        match self {
            Self::Archive(_) => Err(Error::ReadOnly),
            Self::Ext4(file) => file.write_at(offset, bytes),
            Self::Tmpfs(file) => file.write_at(offset, bytes),
        }
    }
    pub fn truncate(&self, size: usize) -> Result<(), Error> {
        match self {
            Self::Archive(_) => Err(Error::ReadOnly),
            Self::Ext4(file) => file.truncate(size),
            Self::Tmpfs(file) => file.truncate(size),
        }
    }
}
