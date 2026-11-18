// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! File namespace and mount ownership.

use crate::{
    Error, ReadAt,
    archive::{self, Archive},
    backend,
    block::Disk,
    ext4::Ext4,
    fat32::Fat32,
    hugepages::Hugepages,
    path::normalize,
    tmpfs::Tmpfs,
};
use alloc::{boxed::Box, collections::BTreeMap, rc::Rc, vec::Vec};
use core::cell::{Cell, RefCell};
use runtime::Handle;

/// Total capacity of all tmpfs and huge page volumes mounted in the namespace.
///
/// Client mounts use a 64 MiB default. The total includes administrative capacities.
const MAX_MEMORY_MOUNT_BYTES: usize = 256 * 1024 * 1024;

pub(crate) struct Namespace {
    archive: Archive,
    cache: crate::cache::Cache,
    next_directory: Cell<u64>,
    mounts: RefCell<BTreeMap<Vec<u8>, MountEntry>>,
    memory_mount_bytes: Cell<usize>,
}

/// A mount table entry and who created it.
struct MountEntry {
    filesystem: Mounted,
    owner: MountOwner,
}

/// Owner of a mount table entry.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum MountOwner {
    /// A client mount created through the mount-control channel.
    User(u64),
    /// A startup or disk mount created by the administrative control.
    Service,
}

type Mounted = Box<dyn backend::Backend>;

type MountedFile = Box<dyn backend::File>;

pub(crate) enum File {
    Archive(archive::File),
    Mounted(MountedFile),
}

pub(crate) struct Directory {
    pub(crate) id: u64,
    node: DirectoryNode,
    parent: crate::Metadata,
    path: Vec<u8>,
}

enum DirectoryNode {
    Archive(archive::Directory),
    Mounted(MountedFile),
}

impl Directory {
    pub(crate) fn statistics(&self) -> Result<crate::Statistics, Error> {
        match &self.node {
            DirectoryNode::Archive(directory) => directory.statistics(),
            DirectoryNode::Mounted(directory) => directory.statistics(),
        }
    }

    pub(crate) fn path(&self) -> Result<Vec<u8>, Error> {
        let mut path = self.path.clone();
        if let DirectoryNode::Mounted(directory) = &self.node {
            let relative = directory.path()?;
            if !relative.is_empty() {
                path.push(b'/');
                path.extend(relative);
            }
        }
        path.insert(0, b'/');
        Ok(path)
    }

    pub(crate) fn fsync(&self) -> Result<(), Error> {
        match &self.node {
            DirectoryNode::Archive(_) => Ok(()),
            DirectoryNode::Mounted(directory) => directory.fsync(),
        }
    }

    pub(crate) fn metadata(&self) -> Result<crate::Metadata, Error> {
        match &self.node {
            DirectoryNode::Archive(directory) => Ok(directory.metadata()),
            DirectoryNode::Mounted(directory) => directory.metadata(),
        }
    }
    pub(crate) fn read_dir(&self) -> Result<Vec<crate::DirEntry>, Error> {
        match &self.node {
            DirectoryNode::Archive(directory) => Ok(directory.read_dir()),
            DirectoryNode::Mounted(directory) => directory.read_dir(),
        }
    }
    pub(crate) fn read_dir_all(&self) -> Result<Vec<crate::DirEntry>, Error> {
        let parent = match &self.node {
            DirectoryNode::Mounted(directory) => {
                directory.parent_metadata()?.unwrap_or(self.parent)
            }
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
    pub(crate) fn set_attributes(&self, attributes: crate::Attributes) -> Result<(), Error> {
        match &self.node {
            DirectoryNode::Archive(_) => Err(Error::ReadOnly),
            DirectoryNode::Mounted(directory) => directory.set_attributes(attributes),
        }
    }
}

impl Namespace {
    pub(crate) fn statistics(&self, path: &[u8]) -> Result<crate::Statistics, Error> {
        self.cache.reclaim()?;
        let path = normalize(&self.resolve(b"/", path, true, false)?)?;
        let mounts = self.mounts.borrow();
        match mounted_at(&mounts, &path) {
            Some((_, filesystem)) => filesystem.statistics(),
            None => self.archive.statistics(),
        }
    }

    pub(crate) fn new(archive: Archive) -> Result<Self, Error> {
        Ok(Self {
            archive,
            cache: crate::cache::Cache::default(),
            next_directory: Cell::new(1),
            mounts: RefCell::new(BTreeMap::new()),
            memory_mount_bytes: Cell::new(0),
        })
    }

    pub(crate) fn mount(
        &self,
        path: &[u8],
        disk: Rc<Disk>,
        start: u64,
        length: u64,
    ) -> Result<(), Error> {
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
        let filesystem: Mounted = if header[1080..1082] == [0x53, 0xef] {
            Box::new(Ext4::mount(disk, start, length)?)
        } else {
            Box::new(Fat32::mount(disk, start, length)?)
        };
        self.install(&path, filesystem, MountOwner::Service)
    }

    pub(crate) fn mount_tmpfs(&self, path: &[u8], capacity: usize) -> Result<(), Error> {
        let volume = if capacity == 0 {
            Tmpfs::new()?
        } else {
            Tmpfs::with_capacity(capacity)?
        };
        self.install(path, Box::new(volume), MountOwner::Service)
    }

    pub(crate) fn mount_tmpfs_user(&self, path: &[u8], channel: u64) -> Result<(), Error> {
        let path = self.check_mount_point(path)?;
        let path = self.check_user_mount_target(path)?;
        self.install(&path, Box::new(Tmpfs::new()?), MountOwner::User(channel))
    }

    pub(crate) fn mount_hugepages(
        &self,
        task: &'static Handle<runtime::Task>,
        path: &[u8],
        owner: MountOwner,
    ) -> Result<(), Error> {
        let path = self.check_mount_point(path)?;
        let path = match owner {
            MountOwner::User(_) => self.check_user_mount_target(path)?,
            MountOwner::Service => path,
        };
        self.install(&path, Box::new(Hugepages::new(task)?), owner)
    }

    fn check_mount_point(&self, path: &[u8]) -> Result<Vec<u8>, Error> {
        let path = normalize(path)?;
        let metadata = self.metadata(&path)?;
        if metadata.mode & 0o170000 != 0o040000 {
            return Err(Error::NotDirectory);
        }
        Ok(path)
    }

    /// Requires a user mount to sit below an existing mount.
    ///
    /// Placing a mount on the read-only boot archive would let a client shadow
    /// system files for every process that resolves through the global namespace.
    fn check_user_mount_target(&self, path: Vec<u8>) -> Result<Vec<u8>, Error> {
        if mounted_at(&self.mounts.borrow(), &path).is_none() {
            return Err(Error::InvalidRequest);
        }
        Ok(path)
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
        let limit = filesystem.memory_limit();
        let total = self
            .memory_mount_bytes
            .get()
            .checked_add(limit)
            .filter(|total| *total <= MAX_MEMORY_MOUNT_BYTES)
            .ok_or(Error::NoSpace)?;
        self.memory_mount_bytes.set(total);
        self.mounts
            .borrow_mut()
            .insert(path, MountEntry { filesystem, owner });
        Ok(())
    }

    pub(crate) fn unmount(&self, path: &[u8]) -> Result<(), Error> {
        self.remove_mount(path, None)
    }

    pub(crate) fn unmount_user_memory(&self, path: &[u8], channel: u64) -> Result<(), Error> {
        self.remove_mount(path, Some(MountOwner::User(channel)))
    }

    pub(crate) fn release_user_mounts(&self, channel: u64) {
        let paths: Vec<_> = self
            .mounts
            .borrow()
            .iter()
            .filter(|(_, entry)| entry.owner == MountOwner::User(channel))
            .map(|(path, _)| path.clone())
            .collect();
        for path in paths {
            if let Some(entry) = self.mounts.borrow_mut().remove(&path) {
                self.memory_mount_bytes.set(
                    self.memory_mount_bytes
                        .get()
                        .saturating_sub(entry.filesystem.memory_limit()),
                );
            }
        }
    }

    fn remove_mount(&self, path: &[u8], owner: Option<MountOwner>) -> Result<(), Error> {
        self.cache.reclaim()?;
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
            if mounts
                .keys()
                .any(|other| other != &path && beneath(other, &path))
            {
                return Err(Error::Busy);
            }
            entry.filesystem.unmount()?;
        }
        if let Some(entry) = self.mounts.borrow_mut().remove(&path) {
            let limit = entry.filesystem.memory_limit();
            self.memory_mount_bytes
                .set(self.memory_mount_bytes.get().saturating_sub(limit));
        }
        Ok(())
    }

    fn mounted<T>(
        &self,
        path: &[u8],
        operation: impl FnOnce(&Mounted, &[u8]) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let path = normalize(path)?;
        let mounts = self.mounts.borrow();
        let (prefix, filesystem) = mounted_at(&mounts, &path).ok_or(Error::ReadOnly)?;
        operation(filesystem, relative(&path, prefix))
    }

    pub(crate) fn set_attributes(
        &self,
        path: &[u8],
        attributes: crate::Attributes,
    ) -> Result<(), Error> {
        self.open_path(path)?.set_attributes(attributes)
    }

    pub(crate) fn reclaim_files(&self) -> Result<(), Error> {
        self.cache.reclaim()
    }

    pub(crate) fn sync(&self) -> Result<(), Error> {
        self.cache.sync()?;
        for entry in self.mounts.borrow().values() {
            entry.filesystem.sync()?;
        }
        Ok(())
    }

    pub(crate) fn create(&self, path: &[u8]) -> Result<File, Error> {
        self.mounted(path, |filesystem, path| {
            filesystem
                .create(path)
                .and_then(|file| self.cache.wrap(file).map(File::Mounted))
        })
    }

    pub(crate) fn open_with(
        &self,
        path: &[u8],
        options: crate::OpenOptions,
    ) -> Result<File, Error> {
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
            Err(Error::NotFound) if options.create => self.mounted(path, |filesystem, path| {
                filesystem
                    .create_with_permissions(path, options.permissions)
                    .and_then(|file| self.cache.wrap(file).map(File::Mounted))
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
    pub(crate) fn mkdir(&self, path: &[u8]) -> Result<(), Error> {
        self.mkdir_with_permissions(path, 0o755)
    }
    pub(crate) fn mkdir_with_permissions(
        &self,
        path: &[u8],
        permissions: u16,
    ) -> Result<(), Error> {
        self.mounted(path, |filesystem, path| {
            filesystem.mkdir_with_permissions(path, permissions)
        })
    }
    pub(crate) fn unlink(&self, path: &[u8]) -> Result<(), Error> {
        let normalized = normalize(path)?;
        if normalized.is_empty() || self.mounts.borrow().contains_key(&normalized) {
            return Err(Error::Busy);
        }
        self.mounted(path, |filesystem, path| filesystem.unlink(path))
    }
    pub(crate) fn read_dir(&self, path: &[u8]) -> Result<Vec<crate::DirEntry>, Error> {
        let path = crate::path::normalize(path)?;
        let mounts = self.mounts.borrow();
        if let Some((prefix, filesystem)) = mounted_at(&mounts, &path) {
            return filesystem.read_dir(relative(&path, prefix));
        }
        let mut entries = self
            .archive
            .read_dir(if path.is_empty() { b"/" } else { &path })?;
        let mut overlays = BTreeMap::new();
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
                overlays.insert(name.to_vec(), entry.filesystem.metadata(b"/")?.inode);
            }
        }
        for (name, inode) in overlays {
            entries.retain(|entry| entry.name != name);
            entries.push(crate::DirEntry {
                name,
                inode,
                kind: crate::FileType::Directory,
            });
        }
        entries.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }
    pub(crate) fn rename(&self, source: &[u8], target: &[u8]) -> Result<(), Error> {
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
        if mounted_at(&mounts, &target).is_none_or(|(target_prefix, _)| target_prefix != prefix) {
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

    pub(crate) fn metadata(&self, path: &[u8]) -> Result<crate::Metadata, Error> {
        let path = normalize(path)?;
        let mounts = self.mounts.borrow();
        if let Some((prefix, filesystem)) = mounted_at(&mounts, &path) {
            return filesystem.metadata(relative(&path, prefix));
        }
        self.archive
            .metadata(if path.is_empty() { b"/" } else { &path })
    }
    pub(crate) fn open_directory(&self, path: &[u8]) -> Result<Directory, Error> {
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
            return filesystem
                .open_directory(relative(&path, prefix))
                .map(|directory| Directory {
                    id,
                    node: DirectoryNode::Mounted(directory),
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
    pub(crate) fn hard_link(&self, source: &[u8], target: &[u8]) -> Result<(), Error> {
        let source = normalize(source)?;
        let target = normalize(target)?;
        let mounts = self.mounts.borrow();
        let Some((prefix, filesystem)) = mounted_at(&mounts, &source) else {
            return Err(Error::ReadOnly);
        };
        if mounted_at(&mounts, &target).is_none_or(|(target_prefix, _)| target_prefix != prefix) {
            return Err(Error::CrossDevice);
        }
        filesystem.hard_link(relative(&source, prefix), relative(&target, prefix))
    }
    pub(crate) fn symlink(&self, target: &[u8], path: &[u8]) -> Result<(), Error> {
        self.mounted(path, |filesystem, path| filesystem.symlink(target, path))
    }
    pub(crate) fn read_link(&self, path: &[u8]) -> Result<Vec<u8>, Error> {
        let path = normalize(path)?;
        let mounts = self.mounts.borrow();
        if let Some((prefix, filesystem)) = mounted_at(&mounts, &path) {
            return filesystem.read_link(relative(&path, prefix));
        }
        self.archive.read_link(&path)
    }

    pub(crate) fn shutdown(&self) -> Result<(), Error> {
        self.cache.sync()?;
        let mut mounts = self.mounts.borrow_mut();
        while let Some(entry) = mounts.first_entry() {
            entry.get().filesystem.unmount()?;
            entry.remove();
        }
        Ok(())
    }

    pub(crate) fn open_path(&self, path: &[u8]) -> Result<File, Error> {
        let path = normalize(path)?;
        let mounts = self.mounts.borrow();
        if let Some((prefix, fs)) = mounted_at(&mounts, &path) {
            return fs
                .open_path(relative(&path, prefix))
                .and_then(|file| self.cache.wrap(file).map(File::Mounted));
        }
        self.archive.open_path(&path).map(File::Archive)
    }

    pub(crate) fn open_bytes(&self, path: &[u8]) -> Result<File, Error> {
        let path = normalize(path)?;
        let mounts = self.mounts.borrow();
        if let Some((prefix, filesystem)) = mounted_at(&mounts, &path) {
            return filesystem
                .open_bytes(relative(&path, prefix))
                .and_then(|file| self.cache.wrap(file).map(File::Mounted));
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

impl ReadAt for File {
    fn len(&self) -> usize {
        match self {
            Self::Archive(file) => file.len(),
            Self::Mounted(file) => file.len(),
        }
    }
    fn read_at(&self, offset: usize, output: &mut [u8]) -> Result<usize, Error> {
        match self {
            Self::Archive(file) => file.read_at(offset, output),
            Self::Mounted(file) => file.read_at(offset, output),
        }
    }
}

impl File {
    pub(crate) fn read_link(&self) -> Result<Vec<u8>, Error> {
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

    pub(crate) fn statistics(&self) -> Result<crate::Statistics, Error> {
        match self {
            Self::Archive(file) => file.statistics(),
            Self::Mounted(file) => file.statistics(),
        }
    }

    pub(crate) fn check_writable(&self) -> Result<(), Error> {
        match self {
            Self::Archive(_) => Err(Error::ReadOnly),
            Self::Mounted(file) => file.check_writable(),
        }
    }

    pub(crate) fn metadata(&self) -> Result<crate::Metadata, Error> {
        match self {
            Self::Archive(file) => Ok(file.metadata()),
            Self::Mounted(file) => file.metadata(),
        }
    }
    pub(crate) fn set_attributes(&self, attributes: crate::Attributes) -> Result<(), Error> {
        match self {
            Self::Archive(_) => Err(Error::ReadOnly),
            Self::Mounted(file) => file.set_attributes(attributes),
        }
    }
    pub(crate) fn append(&self, bytes: &[u8]) -> Result<(usize, usize), Error> {
        match self {
            Self::Archive(_) => Err(Error::ReadOnly),
            Self::Mounted(file) => file.append(bytes),
        }
    }

    pub(crate) fn fsync(&self) -> Result<(), Error> {
        match self {
            Self::Archive(_) => Ok(()),
            Self::Mounted(file) => file.fsync(),
        }
    }
    pub(crate) fn write_at(&self, offset: usize, bytes: &[u8]) -> Result<usize, Error> {
        match self {
            Self::Archive(_) => Err(Error::ReadOnly),
            Self::Mounted(file) => file.write_at(offset, bytes),
        }
    }
    pub(crate) fn truncate(&self, size: usize) -> Result<(), Error> {
        match self {
            Self::Archive(_) => Err(Error::ReadOnly),
            Self::Mounted(file) => file.truncate(size),
        }
    }
    pub(crate) fn mapping_state(
        &self,
        task: &'static Handle<runtime::Task>,
    ) -> Result<Handle<runtime::FrameAllocation>, Error> {
        match self {
            Self::Archive(_) => Err(Error::Unsupported),
            Self::Mounted(file) => file.mapping_state(task),
        }
    }
    pub(crate) fn map_page(
        &self,
        task: &'static Handle<runtime::Task>,
        offset: usize,
        writable: bool,
    ) -> Result<Handle<runtime::FrameAllocation>, Error> {
        match self {
            Self::Archive(_) => Err(Error::Unsupported),
            Self::Mounted(file) => file.map_page(task, offset, writable),
        }
    }
    pub(crate) fn hugepage_size(&self) -> u64 {
        match self {
            Self::Archive(_) => 0,
            Self::Mounted(file) => file.hugepage_size(),
        }
    }
    pub(crate) fn map_huge(
        &self,
        offset: usize,
    ) -> Result<Handle<runtime::FrameAllocation>, Error> {
        match self {
            Self::Archive(_) => Err(Error::Unsupported),
            Self::Mounted(file) => file.map_huge(offset),
        }
    }
}
