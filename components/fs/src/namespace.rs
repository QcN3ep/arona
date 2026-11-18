// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! File namespace and mount ownership.

use crate::{
    Error, FileSystem, ReadAt,
    archive::{self, Archive},
    block::Disk,
    ext4::Ext4,
    fat32::Fat32,
    path::normalize,
    tmpfs::Tmpfs,
};
use alloc::{collections::BTreeMap, rc::Rc, vec::Vec};
use core::cell::{Cell, RefCell};

/// Total capacity of all tmpfs volumes mounted in the namespace.
///
/// Each volume has its own 64 MiB limit; the global limit keeps a client from
/// bypassing that boundary by mounting many small volumes.
const MAX_MEMORY_MOUNT_BYTES: usize = 256 * 1024 * 1024;

pub struct Namespace {
    archive: Archive,
    next_directory: Cell<u64>,
    mounts: RefCell<BTreeMap<Vec<u8>, MountEntry>>,
}

/// A mount table entry and who created it.
struct MountEntry {
    filesystem: Mounted,
    owner: MountOwner,
}

/// Owner of a mount table entry.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum MountOwner {
    /// A client mount created through a specific mount-control channel.
    User(u64),
    /// A startup or disk mount created by the administrative control.
    Service,
}

/// A filesystem mounted at a path in the namespace.
enum Mounted {
    Ext4(Ext4),
    Fat32(Fat32),
    Tmpfs(Tmpfs),
}

impl Mounted {
    /// Capacity this filesystem reserves from the global in-memory volume budget.
    fn memory_limit(&self) -> usize {
        match self {
            Self::Ext4(_) | Self::Fat32(_) => 0,
            Self::Tmpfs(filesystem) => filesystem.limit(),
        }
    }

    fn statistics(&self) -> Result<crate::Statistics, Error> {
        match self {
            Self::Ext4(filesystem) => filesystem.statistics(),
            Self::Fat32(filesystem) => filesystem.statistics(),
            Self::Tmpfs(filesystem) => filesystem.statistics(),
        }
    }

    fn unmount(&self) -> Result<(), Error> {
        match self {
            Self::Ext4(filesystem) => filesystem.unmount(),
            Self::Fat32(filesystem) => filesystem.unmount(),
            Self::Tmpfs(filesystem) => filesystem.unmount(),
        }
    }

    fn metadata(&self, path: &[u8]) -> Result<crate::Metadata, Error> {
        match self {
            Self::Ext4(filesystem) => filesystem.metadata(path),
            Self::Fat32(filesystem) => filesystem.metadata(path),
            Self::Tmpfs(filesystem) => filesystem.metadata(path),
        }
    }

    fn read_dir(&self, path: &[u8]) -> Result<Vec<crate::DirEntry>, Error> {
        match self {
            Self::Ext4(filesystem) => filesystem.read_dir(path),
            Self::Fat32(filesystem) => filesystem.read_dir(path),
            Self::Tmpfs(filesystem) => filesystem.read_dir(path),
        }
    }

    fn rename(&self, source: &[u8], target: &[u8]) -> Result<(), Error> {
        match self {
            Self::Ext4(filesystem) => filesystem.rename(source, target),
            Self::Fat32(filesystem) => filesystem.rename(source, target),
            Self::Tmpfs(filesystem) => filesystem.rename(source, target),
        }
    }

    fn hard_link(&self, source: &[u8], target: &[u8]) -> Result<(), Error> {
        match self {
            Self::Ext4(filesystem) => filesystem.hard_link(source, target),
            Self::Fat32(_) => Err(Error::Unsupported),
            Self::Tmpfs(filesystem) => filesystem.hard_link(source, target),
        }
    }

    fn symlink(&self, target: &[u8], path: &[u8]) -> Result<(), Error> {
        match self {
            Self::Ext4(filesystem) => filesystem.symlink(target, path),
            Self::Fat32(_) => Err(Error::Unsupported),
            Self::Tmpfs(filesystem) => filesystem.symlink(target, path),
        }
    }

    fn read_link(&self, path: &[u8]) -> Result<Vec<u8>, Error> {
        match self {
            Self::Ext4(filesystem) => filesystem.read_link(path),
            Self::Fat32(_) => Err(Error::InvalidRequest),
            Self::Tmpfs(filesystem) => filesystem.read_link(path),
        }
    }

    fn open_path(&self, path: &[u8]) -> Result<File, Error> {
        match self {
            Self::Ext4(filesystem) => filesystem.open_path(path).map(File::Ext4),
            Self::Fat32(filesystem) => filesystem.open_path(path).map(File::Fat32),
            Self::Tmpfs(filesystem) => filesystem.open_path(path).map(File::Tmpfs),
        }
    }

    fn open_bytes(&self, path: &[u8]) -> Result<File, Error> {
        match self {
            Self::Ext4(filesystem) => filesystem.open_bytes(path).map(File::Ext4),
            Self::Fat32(filesystem) => filesystem.open_bytes(path).map(File::Fat32),
            Self::Tmpfs(filesystem) => filesystem.open_bytes(path).map(File::Tmpfs),
        }
    }

    fn open_directory(&self, path: &[u8]) -> Result<DirectoryNode, Error> {
        match self {
            Self::Ext4(filesystem) => filesystem.open_directory(path).map(DirectoryNode::Ext4),
            Self::Fat32(filesystem) => filesystem.open_directory(path).map(DirectoryNode::Fat32),
            Self::Tmpfs(filesystem) => filesystem.open_directory(path).map(DirectoryNode::Tmpfs),
        }
    }

    fn create(&self, path: &[u8]) -> Result<File, Error> {
        match self {
            Self::Ext4(filesystem) => filesystem.create(path).map(File::Ext4),
            Self::Fat32(filesystem) => filesystem.create(path).map(File::Fat32),
            Self::Tmpfs(filesystem) => filesystem.create(path).map(File::Tmpfs),
        }
    }

    fn create_with_permissions(&self, path: &[u8], permissions: u16) -> Result<File, Error> {
        match self {
            Self::Ext4(filesystem) => filesystem
                .create_with_permissions(path, permissions)
                .map(File::Ext4),
            Self::Fat32(filesystem) => filesystem
                .create_with_permissions(path, permissions)
                .map(File::Fat32),
            Self::Tmpfs(filesystem) => filesystem
                .create_with_permissions(path, permissions)
                .map(File::Tmpfs),
        }
    }

    fn mkdir_with_permissions(&self, path: &[u8], permissions: u16) -> Result<(), Error> {
        match self {
            Self::Ext4(filesystem) => filesystem.mkdir_with_permissions(path, permissions),
            Self::Fat32(filesystem) => filesystem.mkdir_with_permissions(path, permissions),
            Self::Tmpfs(filesystem) => filesystem.mkdir_with_permissions(path, permissions),
        }
    }

    fn unlink(&self, path: &[u8]) -> Result<(), Error> {
        match self {
            Self::Ext4(filesystem) => filesystem.unlink(path),
            Self::Fat32(filesystem) => filesystem.unlink(path),
            Self::Tmpfs(filesystem) => filesystem.unlink(path),
        }
    }
}

pub enum File {
    Archive(archive::File),
    Ext4(crate::ext4::File),
    Fat32(crate::fat32::File),
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
    Fat32(crate::fat32::File),
    Tmpfs(crate::tmpfs::File),
}

impl Directory {
    pub fn statistics(&self) -> Result<crate::Statistics, Error> {
        match &self.node {
            DirectoryNode::Archive(directory) => directory.statistics(),
            DirectoryNode::Ext4(directory) => directory.statistics(),
            DirectoryNode::Fat32(directory) => directory.statistics(),
            DirectoryNode::Tmpfs(directory) => directory.statistics(),
        }
    }

    pub fn path(&self) -> Result<Vec<u8>, Error> {
        let mut path = self.path.clone();
        let relative = match &self.node {
            DirectoryNode::Archive(_) => Vec::new(),
            DirectoryNode::Ext4(directory) => directory.path()?,
            DirectoryNode::Fat32(directory) => directory.path()?,
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
            DirectoryNode::Fat32(directory) => directory.fsync(),
            DirectoryNode::Tmpfs(directory) => directory.fsync(),
        }
    }

    pub fn metadata(&self) -> Result<crate::Metadata, Error> {
        match &self.node {
            DirectoryNode::Archive(directory) => Ok(directory.metadata()),
            DirectoryNode::Ext4(directory) => directory.metadata(),
            DirectoryNode::Fat32(directory) => directory.metadata(),
            DirectoryNode::Tmpfs(directory) => directory.metadata(),
        }
    }
    pub fn read_dir(&self) -> Result<Vec<crate::DirEntry>, Error> {
        match &self.node {
            DirectoryNode::Archive(directory) => Ok(directory.read_dir()),
            DirectoryNode::Ext4(directory) => directory.read_dir(),
            DirectoryNode::Fat32(directory) => directory.read_dir(),
            DirectoryNode::Tmpfs(directory) => directory.read_dir(),
        }
    }
    pub fn read_dir_all(&self) -> Result<Vec<crate::DirEntry>, Error> {
        let parent = match &self.node {
            DirectoryNode::Ext4(directory) => directory.parent_metadata()?.unwrap_or(self.parent),
            DirectoryNode::Fat32(directory) => directory.parent_metadata()?.unwrap_or(self.parent),
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
            DirectoryNode::Fat32(directory) => directory.set_attributes(attributes),
            DirectoryNode::Tmpfs(directory) => directory.set_attributes(attributes),
        }
    }
}

impl Namespace {
    pub fn statistics(&self, path: &[u8]) -> Result<crate::Statistics, Error> {
        let path = normalize(&self.resolve(b"/", path, true, false)?)?;
        let mounts = self.mounts.borrow();
        if let Some((_, filesystem)) = mounted_at(&mounts, &path) {
            return filesystem.statistics();
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
        self.check_install(&path)?;
        if length < 4096
            || start
                .checked_add(length)
                .is_none_or(|end| end > disk.capacity())
        {
            return Err(Error::InvalidData);
        }
        let mut header = [0; 4096];
        disk.read(start, &mut header)?;
        let filesystem = if header[1080..1082] == [0x53, 0xef] {
            Mounted::Ext4(Ext4::mount(disk, start, length)?)
        } else {
            Mounted::Fat32(Fat32::mount(disk, start, length)?)
        };
        self.install(&path, filesystem, MountOwner::Service)
    }

    /// Mounts a fresh in-memory filesystem at a path.
    pub fn mount_tmpfs(&self, path: &[u8], capacity: usize) -> Result<(), Error> {
        let path = normalize(path)?;
        self.check_install(&path)?;
        let filesystem = if capacity == 0 {
            Tmpfs::new()?
        } else {
            Tmpfs::with_capacity(capacity)?
        };
        self.install(&path, Mounted::Tmpfs(filesystem), MountOwner::Service)
    }

    /// Mounts a tmpfs on an existing directory for a user mount request.
    pub fn mount_user_tmpfs(&self, path: &[u8], channel: u64) -> Result<(), Error> {
        let path = normalize(path)?;
        let metadata = self.metadata(&path)?;
        if metadata.mode & 0o170000 != 0o040000 {
            return Err(Error::NotDirectory);
        }
        // A user mount must sit at or below an existing mount. Placing one in the
        // read-only boot archive would let a client shadow system files for every
        // process that resolves through the global namespace.
        if mounted_at(&self.mounts.borrow(), &path).is_none() {
            return Err(Error::InvalidRequest);
        }
        self.install(
            &path,
            Mounted::Tmpfs(Tmpfs::new()?),
            MountOwner::User(channel),
        )
    }

    fn check_install(&self, path: &[u8]) -> Result<(), Error> {
        let mounts = self.mounts.borrow();
        if path.is_empty()
            || mounts.contains_key(path)
            || mounts.keys().any(|other| beneath(other, path))
        {
            return Err(Error::Busy);
        }
        Ok(())
    }

    fn install(&self, path: &[u8], filesystem: Mounted, owner: MountOwner) -> Result<(), Error> {
        let path = normalize(path)?;
        self.check_install(&path)?;
        let mounts = self.mounts.borrow();
        let total = mounts
            .values()
            .map(|entry| entry.filesystem.memory_limit())
            .sum::<usize>()
            .checked_add(filesystem.memory_limit())
            .ok_or(Error::NoSpace)?;
        if total > MAX_MEMORY_MOUNT_BYTES {
            return Err(Error::NoSpace);
        }
        drop(mounts);
        self.mounts
            .borrow_mut()
            .insert(path, MountEntry { filesystem, owner });

        Ok(())
    }

    /// Unmounts a service volume after every nested mount has been removed.
    pub fn unmount(&self, path: &[u8]) -> Result<(), Error> {
        self.remove_mount(path, None)
    }

    pub fn unmount_user_tmpfs(&self, path: &[u8], channel: u64) -> Result<(), Error> {
        self.remove_mount(path, Some(MountOwner::User(channel)))
    }

    /// Drops every mount the given mount-control channel created.
    ///
    /// Open sessions keep their volume alive, so this only removes the table
    /// entries and their budget charge. The channel's owner is exiting and its
    /// guests are gone, so there is no need to synchronously unmount the volumes.
    pub fn release_user_mounts(&self, channel: u64) {
        let paths: Vec<Vec<u8>> = self
            .mounts
            .borrow()
            .iter()
            .filter(|(_, entry)| entry.owner == MountOwner::User(channel))
            .map(|(path, _)| path.clone())
            .collect();
        self.release_paths(paths);
    }

    fn release_paths(&self, mut paths: Vec<Vec<u8>>) {
        paths.sort_by_key(|path| core::cmp::Reverse(path.len()));
        for path in paths {
            self.mounts.borrow_mut().remove(&path);
        }
    }

    fn remove_mount(&self, path: &[u8], owner: Option<MountOwner>) -> Result<(), Error> {
        let path = normalize(path)?;
        {
            let mounts = self.mounts.borrow();
            let Some(entry) = mounts.get(&path) else {
                drop(mounts);
                return match self.metadata(&path) {
                    Err(Error::NotFound) => Err(Error::NotFound),
                    _ => Err(Error::InvalidRequest),
                };
            };
            if owner.is_some_and(|owner| entry.owner != owner) {
                return Err(Error::InvalidRequest);
            }
        }
        {
            let mounts = self.mounts.borrow();
            let entry = mounts.get(&path).ok_or(Error::NotFound)?;
            if mounts
                .keys()
                .any(|other| other != &path && beneath(other, &path))
            {
                return Err(Error::Busy);
            }
            entry.filesystem.unmount()?;
        }
        self.mounts.borrow_mut().remove(&path);
        Ok(())
    }

    fn mounted<T>(
        &self,
        path: &[u8],
        operation: impl FnOnce(&Mounted, &[u8]) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let path = crate::path::normalize(path)?;
        let mounts = self.mounts.borrow();
        let Some((prefix, filesystem)) = mounted_at(&mounts, &path) else {
            return Err(Error::ReadOnly);
        };
        operation(filesystem, relative(&path, prefix))
    }

    pub fn create(&self, path: &[u8]) -> Result<File, Error> {
        self.mounted(path, Mounted::create)
    }

    pub fn open_with(&self, path: &[u8], options: crate::OpenOptions) -> Result<File, Error> {
        options.validate()?;
        let file = match if options.path_only {
            self.open_path(path)
        } else {
            self.open_bytes(path)
        } {
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
        if let Some((prefix, filesystem)) = mounted_at(&mounts, &path) {
            return filesystem.read_dir(relative(&path, prefix));
        }
        let mut entries = self
            .archive
            .read_dir(if path.is_empty() { b"/" } else { &path })?;
        for (prefix, entry) in mounts.iter() {
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
                    inode: entry.filesystem.metadata(b"/")?.inode,
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
        let mounts = self.mounts.borrow();
        let Some((prefix, filesystem)) = mounted_at(&mounts, &source) else {
            return Err(Error::ReadOnly);
        };
        if !beneath(&target, prefix)
            || mounted_at(&mounts, &target).is_none_or(|(target, _)| target != prefix)
        {
            return Err(Error::CrossDevice);
        }
        if mounts
            .keys()
            .any(|other| beneath(other, &source) || beneath(other, &target))
        {
            return Err(Error::Busy);
        }
        filesystem.rename(relative(&source, prefix), relative(&target, prefix))
    }

    pub fn metadata(&self, path: &[u8]) -> Result<crate::Metadata, Error> {
        let path = normalize(path)?;
        let mounts = self.mounts.borrow();
        if let Some((prefix, filesystem)) = mounted_at(&mounts, &path) {
            return filesystem.metadata(relative(&path, prefix));
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
        let mounts = self.mounts.borrow();
        if let Some((prefix, filesystem)) = mounted_at(&mounts, &path) {
            let node = filesystem.open_directory(relative(&path, prefix))?;
            return Ok(Directory {
                id,
                node,
                parent,
                path: prefix.to_vec(),
            });
        }
        drop(mounts);
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
        let mounts = self.mounts.borrow();
        let Some((prefix, filesystem)) = mounted_at(&mounts, &source) else {
            return Err(Error::ReadOnly);
        };
        if !beneath(&target, prefix)
            || mounted_at(&mounts, &target).is_none_or(|(target, _)| target != prefix)
        {
            return Err(Error::CrossDevice);
        }
        filesystem.hard_link(relative(&source, prefix), relative(&target, prefix))
    }
    pub fn symlink(&self, target: &[u8], path: &[u8]) -> Result<(), Error> {
        self.mounted(path, |fs, path| fs.symlink(target, path))
    }
    pub fn read_link(&self, path: &[u8]) -> Result<Vec<u8>, Error> {
        let path = normalize(path)?;
        let mounts = self.mounts.borrow();
        if let Some((prefix, filesystem)) = mounted_at(&mounts, &path) {
            return filesystem.read_link(relative(&path, prefix));
        }
        self.archive.read_link(&path)
    }

    pub fn set_attributes(&self, path: &[u8], attributes: crate::Attributes) -> Result<(), Error> {
        self.mounted(path, |fs, path| {
            fs.open_path(path)?.set_attributes(attributes)
        })
    }

    /// Flushes every mounted filesystem; the read-only archive needs no flush.
    pub fn sync(&self) -> Result<(), Error> {
        for entry in self.mounts.borrow().values() {
            match &entry.filesystem {
                Mounted::Ext4(filesystem) => filesystem.sync()?,
                Mounted::Fat32(filesystem) => filesystem.sync()?,
                Mounted::Tmpfs(_) => (),
            }
        }
        Ok(())
    }

    pub fn shutdown(&self) -> Result<(), Error> {
        let mut mounts = self.mounts.borrow_mut();
        while let Some(entry) = mounts.first_entry() {
            entry.get().filesystem.unmount()?;
            entry.remove();
        }
        Ok(())
    }

    pub fn open_path(&self, path: &[u8]) -> Result<File, Error> {
        let path = normalize(path)?;
        let mounts = self.mounts.borrow();
        if let Some((prefix, filesystem)) = mounted_at(&mounts, &path) {
            return filesystem.open_path(relative(&path, prefix));
        }
        self.archive.open_path(&path).map(File::Archive)
    }

    pub fn open_bytes(&self, path: &[u8]) -> Result<File, Error> {
        let path = normalize(path)?;
        let mounts = self.mounts.borrow();
        if let Some((prefix, filesystem)) = mounted_at(&mounts, &path) {
            return filesystem.open_bytes(relative(&path, prefix));
        }
        self.archive.open_bytes(&path).map(File::Archive)
    }
}

fn mounted_at<'a>(
    mounts: &'a BTreeMap<Vec<u8>, MountEntry>,
    path: &[u8],
) -> Option<(&'a [u8], &'a Mounted)> {
    mounts
        .iter()
        .rev()
        .find(|(prefix, _)| beneath(path, prefix))
        .map(|(prefix, entry)| (prefix.as_slice(), &entry.filesystem))
}

fn relative<'a>(path: &'a [u8], prefix: &[u8]) -> &'a [u8] {
    path.get(prefix.len() + 1..).unwrap_or(b"/")
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
            Self::Fat32(file) => file.len(),
            Self::Tmpfs(file) => file.len(),
        }
    }
    fn read_at(&self, offset: usize, output: &mut [u8]) -> Result<usize, Error> {
        match self {
            Self::Archive(file) => file.read_at(offset, output),
            Self::Ext4(file) => file.read_at(offset, output),
            Self::Fat32(file) => file.read_at(offset, output),
            Self::Tmpfs(file) => file.read_at(offset, output),
        }
    }
}

impl File {
    pub fn read_link(&self) -> Result<Vec<u8>, Error> {
        let info = self.metadata()?;
        if info.mode & 0o170000 != 0o120000 {
            return Err(Error::InvalidRequest);
        }
        if info.size >= 4096 {
            return Err(Error::InvalidData);
        }
        let mut bytes = alloc::vec![0; info.size as usize];
        self.read_exact_at(0, &mut bytes)?;
        Ok(bytes)
    }

    pub fn statistics(&self) -> Result<crate::Statistics, Error> {
        match self {
            Self::Archive(file) => file.statistics(),
            Self::Ext4(file) => file.statistics(),
            Self::Fat32(file) => file.statistics(),
            Self::Tmpfs(file) => file.statistics(),
        }
    }

    pub fn check_writable(&self) -> Result<(), Error> {
        match self {
            Self::Archive(_) => Err(Error::ReadOnly),
            Self::Ext4(file) => file.check_writable(),
            Self::Fat32(file) => file.check_writable(),
            Self::Tmpfs(file) => file.check_writable(),
        }
    }

    pub fn metadata(&self) -> Result<crate::Metadata, Error> {
        match self {
            Self::Archive(file) => Ok(file.metadata()),
            Self::Ext4(file) => file.metadata(),
            Self::Fat32(file) => file.metadata(),
            Self::Tmpfs(file) => file.metadata(),
        }
    }
    pub fn set_attributes(&self, attributes: crate::Attributes) -> Result<(), Error> {
        match self {
            Self::Archive(_) => Err(Error::ReadOnly),
            Self::Ext4(file) => file.set_attributes(attributes),
            Self::Fat32(file) => file.set_attributes(attributes),
            Self::Tmpfs(file) => file.set_attributes(attributes),
        }
    }
    pub fn append(&self, bytes: &[u8]) -> Result<(usize, usize), Error> {
        match self {
            Self::Archive(_) => Err(Error::ReadOnly),
            Self::Ext4(file) => file.append(bytes),
            Self::Fat32(file) => file.append(bytes),
            Self::Tmpfs(file) => file.append(bytes),
        }
    }

    pub fn fsync(&self) -> Result<(), Error> {
        match self {
            Self::Archive(_) => Ok(()),
            Self::Ext4(file) => file.fsync(),
            Self::Fat32(file) => file.fsync(),
            Self::Tmpfs(file) => file.fsync(),
        }
    }
    pub fn write_at(&self, offset: usize, bytes: &[u8]) -> Result<usize, Error> {
        match self {
            Self::Archive(_) => Err(Error::ReadOnly),
            Self::Ext4(file) => file.write_at(offset, bytes),
            Self::Fat32(file) => file.write_at(offset, bytes),
            Self::Tmpfs(file) => file.write_at(offset, bytes),
        }
    }
    pub fn truncate(&self, size: usize) -> Result<(), Error> {
        match self {
            Self::Archive(_) => Err(Error::ReadOnly),
            Self::Ext4(file) => file.truncate(size),
            Self::Fat32(file) => file.truncate(size),
            Self::Tmpfs(file) => file.truncate(size),
        }
    }
}
