// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! In-memory filesystem backed by neither a block device nor the archive.

use crate::metadata::{device_identity, mount_identity};
use crate::path::components;
use crate::{Attributes, DirEntry, Error, FileType, Metadata, ReadAt, Statistics, Timestamp};
use alloc::{collections::BTreeMap, rc::Rc, vec::Vec};
use core::cell::RefCell;

const BLOCK_SIZE: usize = 4096;
const MAX_FILE_SIZE: usize = 2 * 1024 * 1024 * 1024;
const NAME_LENGTH: u64 = 255;
const DEFAULT_CAPACITY: usize = 64 * 1024 * 1024;
const FREE_INODES: u64 = 1 << 20;
const TMPFS_MAGIC: u64 = 0x0102_1994;
const ROOT: u64 = 1;

pub struct Tmpfs {
    volume: Rc<RefCell<Volume>>,
}

pub struct File {
    volume: Rc<RefCell<Volume>>,
    inode: u64,
}

struct Volume {
    device: u64,
    mount: u64,
    capacity: usize,
    next_inode: u64,
    nodes: BTreeMap<u64, Node>,
    open: BTreeMap<u64, usize>,
}

struct Node {
    mode: u32,
    uid: u32,
    gid: u32,
    links: u32,
    parent: u64,
    data: Vec<u8>,
    target: Vec<u8>,
    children: BTreeMap<Vec<u8>, u64>,
    accessed: Timestamp,
    modified: Timestamp,
    changed: Timestamp,
    birth: Timestamp,
}

impl Tmpfs {
    pub fn new() -> Result<Self, Error> {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Result<Self, Error> {
        if capacity < BLOCK_SIZE || capacity % BLOCK_SIZE != 0 {
            return Err(Error::InvalidRequest);
        }
        let mut nodes = BTreeMap::new();
        nodes.insert(
            ROOT,
            Node {
                mode: 0o040755,
                uid: 0,
                gid: 0,
                links: 2,
                parent: ROOT,
                data: Vec::new(),
                target: Vec::new(),
                children: BTreeMap::new(),
                accessed: Timestamp::default(),
                modified: Timestamp::default(),
                changed: Timestamp::default(),
                birth: Timestamp::default(),
            },
        );
        Ok(Self {
            volume: Rc::new(RefCell::new(Volume {
                device: device_identity()?,
                mount: mount_identity()?,
                capacity,
                next_inode: ROOT + 1,
                nodes,
                open: BTreeMap::new(),
            })),
        })
    }

    pub fn statistics(&self) -> Result<Statistics, Error> {
        self.volume.borrow().statistics()
    }

    pub fn busy(&self) -> bool {
        Rc::strong_count(&self.volume) != 1
    }

    pub fn unmount(&self) -> Result<(), Error> {
        if self.busy() {
            return Err(Error::Busy);
        }
        Ok(())
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
        let node = volume.nodes.get(&inode).ok_or(Error::InvalidData)?;
        if node.is_directory() != directory {
            return Err(if directory {
                Error::NotDirectory
            } else {
                Error::IsDirectory
            });
        }
        if node.is_symlink() {
            return Err(Error::InvalidRequest);
        }
        let count = volume.open.entry(inode).or_insert(0);
        *count = count.checked_add(1).ok_or(Error::NoSpace)?;
        Ok(File {
            volume: Rc::clone(&self.volume),
            inode,
        })
    }

    pub fn create(&self, path: &[u8]) -> Result<File, Error> {
        self.create_with_permissions(path, 0o644)
    }

    pub fn create_with_permissions(&self, path: &[u8], permissions: u16) -> Result<File, Error> {
        if permissions & !0o7777 != 0 {
            return Err(Error::InvalidRequest);
        }
        self.volume
            .borrow_mut()
            .create(path, false, u32::from(permissions))?;
        self.open_bytes(path)
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
            .create(path, true, u32::from(permissions))
            .map(|_| ())
    }

    pub fn metadata(&self, path: &[u8]) -> Result<Metadata, Error> {
        let volume = self.volume.borrow();
        volume.metadata(volume.lookup(path)?)
    }

    pub fn read_dir(&self, path: &[u8]) -> Result<Vec<DirEntry>, Error> {
        let volume = self.volume.borrow();
        volume.read_dir(volume.lookup(path)?)
    }

    pub fn unlink(&self, path: &[u8]) -> Result<(), Error> {
        self.volume.borrow_mut().unlink(path)
    }

    pub fn rename(&self, source: &[u8], target: &[u8]) -> Result<(), Error> {
        self.volume.borrow_mut().rename(source, target)
    }

    pub fn hard_link(&self, source: &[u8], target: &[u8]) -> Result<(), Error> {
        self.volume.borrow_mut().hard_link(source, target)
    }

    pub fn symlink(&self, target: &[u8], path: &[u8]) -> Result<(), Error> {
        self.volume.borrow_mut().symlink(target, path)
    }

    pub fn read_link(&self, path: &[u8]) -> Result<Vec<u8>, Error> {
        let volume = self.volume.borrow();
        let inode = volume.lookup(path)?;
        let node = volume.nodes.get(&inode).ok_or(Error::InvalidData)?;
        if !node.is_symlink() {
            return Err(Error::InvalidRequest);
        }
        Ok(node.target.clone())
    }
}

impl Volume {
    fn statistics(&self) -> Result<Statistics, Error> {
        let used = self.used_blocks();
        let total = self.capacity / BLOCK_SIZE;
        Ok(Statistics {
            kind: TMPFS_MAGIC,
            device: self.device,
            block_size: BLOCK_SIZE as u64,
            blocks: total as u64,
            free_blocks: total.saturating_sub(used) as u64,
            available_blocks: total.saturating_sub(used) as u64,
            files: FREE_INODES + self.nodes.len() as u64,
            free_files: FREE_INODES,
            name_length: NAME_LENGTH,
            flags: 0,
        })
    }

    fn used_blocks(&self) -> usize {
        self.nodes
            .values()
            .filter(|node| !node.is_directory() && !node.is_symlink())
            .map(|node| node.data.len().div_ceil(BLOCK_SIZE))
            .sum()
    }

    fn lookup(&self, path: &[u8]) -> Result<u64, Error> {
        let mut inode = ROOT;
        for name in components(path)? {
            let node = self.nodes.get(&inode).ok_or(Error::InvalidData)?;
            if !node.is_directory() {
                return Err(Error::NotDirectory);
            }
            inode = *node.children.get(name).ok_or(Error::NotFound)?;
        }
        Ok(inode)
    }

    fn parent(&self, path: &[u8]) -> Result<(u64, Vec<u8>), Error> {
        let parts = components(path)?;
        let (name, parents) = parts.split_last().ok_or(Error::InvalidRequest)?;
        let mut inode = ROOT;
        for part in parents {
            let node = self.nodes.get(&inode).ok_or(Error::InvalidData)?;
            if !node.is_directory() {
                return Err(Error::NotDirectory);
            }
            inode = *node.children.get(*part).ok_or(Error::NotFound)?;
        }
        let node = self.nodes.get(&inode).ok_or(Error::InvalidData)?;
        if !node.is_directory() {
            return Err(Error::NotDirectory);
        }
        Ok((inode, name.to_vec()))
    }

    fn create(&mut self, path: &[u8], directory: bool, permissions: u32) -> Result<u64, Error> {
        let (parent, name) = self.parent(path)?;
        if let Some(node) = self.nodes.get(&parent) {
            if node.children.contains_key(&name) {
                return Err(Error::AlreadyExists);
            }
        } else {
            return Err(Error::InvalidData);
        }
        let inode = self.next_inode;
        self.next_inode = self.next_inode.checked_add(1).ok_or(Error::NoSpace)?;
        self.nodes.insert(
            inode,
            Node {
                mode: if directory { 0o040000 } else { 0o100000 } | permissions,
                uid: 0,
                gid: 0,
                links: if directory { 2 } else { 1 },
                parent,
                data: Vec::new(),
                target: Vec::new(),
                children: BTreeMap::new(),
                accessed: Timestamp::default(),
                modified: Timestamp::default(),
                changed: Timestamp::default(),
                birth: Timestamp::default(),
            },
        );
        let parent_node = self.nodes.get_mut(&parent).ok_or(Error::InvalidData)?;
        parent_node.children.insert(name, inode);
        if directory {
            parent_node.links = parent_node.links.checked_add(1).ok_or(Error::NoSpace)?;
        }
        Ok(inode)
    }

    fn unlink(&mut self, path: &[u8]) -> Result<(), Error> {
        let (parent, name) = self.parent(path)?;
        let inode = {
            let node = self.nodes.get(&parent).ok_or(Error::InvalidData)?;
            *node.children.get(&name).ok_or(Error::NotFound)?
        };
        let directory = {
            let node = self.nodes.get(&inode).ok_or(Error::InvalidData)?;
            if node.is_directory() && !node.children.is_empty() {
                return Err(Error::NotEmpty);
            }
            node.is_directory()
        };
        let parent_node = self.nodes.get_mut(&parent).ok_or(Error::InvalidData)?;
        parent_node.children.remove(&name);
        if directory {
            parent_node.links = parent_node.links.checked_sub(1).ok_or(Error::InvalidData)?;
        }
        self.release(inode)
    }

    /// Removes an entry link and frees the inode after its last session closes.
    fn release(&mut self, inode: u64) -> Result<(), Error> {
        let links = {
            let node = self.nodes.get_mut(&inode).ok_or(Error::InvalidData)?;
            node.links = if node.is_directory() {
                0
            } else {
                node.links.checked_sub(1).ok_or(Error::InvalidData)?
            };
            node.links
        };
        if links == 0 && !self.open.contains_key(&inode) {
            self.nodes.remove(&inode);
        }
        Ok(())
    }

    fn rename(&mut self, source: &[u8], target: &[u8]) -> Result<(), Error> {
        let (old_parent, old_name) = self.parent(source)?;
        let (new_parent, new_name) = self.parent(target)?;
        let inode = {
            let node = self.nodes.get(&old_parent).ok_or(Error::InvalidData)?;
            *node.children.get(&old_name).ok_or(Error::NotFound)?
        };
        if old_parent == new_parent && old_name == new_name {
            return Ok(());
        }
        let replaced = {
            let node = self.nodes.get(&new_parent).ok_or(Error::InvalidData)?;
            node.children.get(&new_name).copied()
        };
        if replaced == Some(inode) {
            return Ok(());
        }
        let directory = self
            .nodes
            .get(&inode)
            .ok_or(Error::InvalidData)?
            .is_directory();
        if let Some(other) = replaced {
            let other_node = self.nodes.get(&other).ok_or(Error::InvalidData)?;
            let node = self.nodes.get(&inode).ok_or(Error::InvalidData)?;
            if other_node.is_directory() != node.is_directory() {
                return Err(if other_node.is_directory() {
                    Error::IsDirectory
                } else {
                    Error::NotDirectory
                });
            }
            if other_node.is_directory() && !other_node.children.is_empty() {
                return Err(Error::NotEmpty);
            }
        }
        if directory {
            self.check_ancestry(inode, new_parent)?;
        }
        if let Some(other) = replaced {
            self.detach(other, new_parent, &new_name)?;
        }
        let old_node = self.nodes.get_mut(&old_parent).ok_or(Error::InvalidData)?;
        old_node.children.remove(&old_name);
        let new_node = self.nodes.get_mut(&new_parent).ok_or(Error::InvalidData)?;
        new_node.children.insert(new_name, inode);
        let node = self.nodes.get_mut(&inode).ok_or(Error::InvalidData)?;
        node.parent = new_parent;
        if directory && old_parent != new_parent {
            let old_node = self.nodes.get_mut(&old_parent).ok_or(Error::InvalidData)?;
            old_node.links = old_node.links.checked_sub(1).ok_or(Error::InvalidData)?;
            let new_node = self.nodes.get_mut(&new_parent).ok_or(Error::InvalidData)?;
            new_node.links = new_node.links.checked_add(1).ok_or(Error::NoSpace)?;
        }
        Ok(())
    }

    /// Removes the existing entry a rename replaces.
    fn detach(&mut self, inode: u64, parent: u64, name: &[u8]) -> Result<(), Error> {
        let directory = {
            let node = self.nodes.get(&inode).ok_or(Error::InvalidData)?;
            node.is_directory()
        };
        let parent_node = self.nodes.get_mut(&parent).ok_or(Error::InvalidData)?;
        parent_node.children.remove(name);
        if directory {
            parent_node.links = parent_node.links.checked_sub(1).ok_or(Error::InvalidData)?;
        }
        self.release(inode)
    }

    /// Rejects renaming a directory beneath itself.
    fn check_ancestry(&self, inode: u64, mut directory: u64) -> Result<(), Error> {
        while directory != ROOT {
            if directory == inode {
                return Err(Error::InvalidRequest);
            }
            directory = self.nodes.get(&directory).ok_or(Error::InvalidData)?.parent;
        }
        Ok(())
    }

    fn hard_link(&mut self, source: &[u8], target: &[u8]) -> Result<(), Error> {
        let inode = self.lookup(source)?;
        if self
            .nodes
            .get(&inode)
            .ok_or(Error::InvalidData)?
            .is_directory()
        {
            return Err(Error::IsDirectory);
        }
        let (parent, name) = self.parent(target)?;
        {
            let node = self.nodes.get(&parent).ok_or(Error::InvalidData)?;
            if node.children.contains_key(&name) {
                return Err(Error::AlreadyExists);
            }
        }
        let node = self.nodes.get_mut(&inode).ok_or(Error::InvalidData)?;
        node.links = node.links.checked_add(1).ok_or(Error::NoSpace)?;
        let parent_node = self.nodes.get_mut(&parent).ok_or(Error::InvalidData)?;
        parent_node.children.insert(name, inode);
        Ok(())
    }

    fn symlink(&mut self, target: &[u8], path: &[u8]) -> Result<(), Error> {
        if target.is_empty() || target.len() > NAME_LENGTH as usize || target.contains(&0) {
            return Err(Error::InvalidRequest);
        }
        let (parent, name) = self.parent(path)?;
        {
            let node = self.nodes.get(&parent).ok_or(Error::InvalidData)?;
            if node.children.contains_key(&name) {
                return Err(Error::AlreadyExists);
            }
        }
        let inode = self.next_inode;
        self.next_inode = self.next_inode.checked_add(1).ok_or(Error::NoSpace)?;
        self.nodes.insert(
            inode,
            Node {
                mode: 0o120777,
                uid: 0,
                gid: 0,
                links: 1,
                parent,
                data: Vec::new(),
                target: target.to_vec(),
                children: BTreeMap::new(),
                accessed: Timestamp::default(),
                modified: Timestamp::default(),
                changed: Timestamp::default(),
                birth: Timestamp::default(),
            },
        );
        let parent_node = self.nodes.get_mut(&parent).ok_or(Error::InvalidData)?;
        parent_node.children.insert(name, inode);
        Ok(())
    }

    fn metadata(&self, inode: u64) -> Result<Metadata, Error> {
        let node = self.nodes.get(&inode).ok_or(Error::InvalidData)?;
        let size = if node.is_symlink() {
            node.target.len() as u64
        } else {
            node.data.len() as u64
        };
        Ok(Metadata {
            device: self.device,
            mount: self.mount,
            inode,
            size,
            blocks: if node.is_directory() || node.is_symlink() {
                0
            } else {
                (node.data.len().div_ceil(BLOCK_SIZE) * (BLOCK_SIZE / 512)) as u64
            },
            links: node.links,
            mode: node.mode,
            uid: node.uid,
            gid: node.gid,
            accessed: node.accessed,
            modified: node.modified,
            changed: node.changed,
            birth: node.birth,
        })
    }

    fn read_dir(&self, inode: u64) -> Result<Vec<DirEntry>, Error> {
        let node = self.nodes.get(&inode).ok_or(Error::InvalidData)?;
        if !node.is_directory() {
            return Err(Error::NotDirectory);
        }
        let mut entries = Vec::with_capacity(node.children.len());
        for (name, inode) in &node.children {
            let child = self.nodes.get(inode).ok_or(Error::InvalidData)?;
            entries.push(DirEntry {
                name: name.clone(),
                inode: *inode,
                kind: child.file_type(),
            });
        }
        Ok(entries)
    }

    fn check_growth(&self, inode: u64, size: usize) -> Result<(), Error> {
        if size > MAX_FILE_SIZE {
            return Err(Error::FileTooLarge);
        }
        let current = self.nodes.get(&inode).ok_or(Error::InvalidData)?.data.len();
        let before = current.div_ceil(BLOCK_SIZE);
        let after = size.div_ceil(BLOCK_SIZE);
        if after <= before {
            return Ok(());
        }
        let total = self.capacity / BLOCK_SIZE;
        if self.used_blocks() - before + after > total {
            return Err(Error::NoSpace);
        }
        Ok(())
    }
}

impl File {
    pub fn statistics(&self) -> Result<Statistics, Error> {
        self.volume.borrow().statistics()
    }

    pub fn path(&self) -> Result<Vec<u8>, Error> {
        self.volume.borrow().path(self.inode)
    }

    pub fn check_writable(&self) -> Result<(), Error> {
        Ok(())
    }

    pub fn read_dir(&self) -> Result<Vec<DirEntry>, Error> {
        self.volume.borrow().read_dir(self.inode)
    }

    pub fn metadata(&self) -> Result<Metadata, Error> {
        self.volume.borrow().metadata(self.inode)
    }

    pub fn parent_metadata(&self) -> Result<Option<Metadata>, Error> {
        let volume = self.volume.borrow();
        let node = volume.nodes.get(&self.inode).ok_or(Error::InvalidData)?;
        if self.inode == ROOT {
            return Ok(None);
        }
        volume.metadata(node.parent).map(Some)
    }

    pub fn set_attributes(&self, attributes: Attributes) -> Result<(), Error> {
        attributes.validate()?;
        let mut volume = self.volume.borrow_mut();
        let node = volume
            .nodes
            .get_mut(&self.inode)
            .ok_or(Error::InvalidData)?;
        node.mode = (node.mode & 0o170000) | attributes.permissions;
        node.uid = attributes.uid;
        node.gid = attributes.gid;
        node.accessed = attributes.accessed;
        node.modified = attributes.modified;
        node.changed = attributes.changed;
        Ok(())
    }

    pub fn append(&self, bytes: &[u8]) -> Result<(usize, usize), Error> {
        let mut volume = self.volume.borrow_mut();
        let offset = volume
            .nodes
            .get(&self.inode)
            .ok_or(Error::InvalidData)?
            .data
            .len();
        let end = offset.checked_add(bytes.len()).ok_or(Error::FileTooLarge)?;
        volume.check_growth(self.inode, end)?;
        let node = volume
            .nodes
            .get_mut(&self.inode)
            .ok_or(Error::InvalidData)?;
        if node.is_directory() {
            return Err(Error::IsDirectory);
        }
        node.data
            .try_reserve(bytes.len())
            .map_err(|_| Error::NoSpace)?;
        node.data.extend_from_slice(bytes);
        Ok((offset, bytes.len()))
    }

    pub fn write_at(&self, offset: usize, bytes: &[u8]) -> Result<usize, Error> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let mut volume = self.volume.borrow_mut();
        let end = offset.checked_add(bytes.len()).ok_or(Error::FileTooLarge)?;
        volume.check_growth(self.inode, end)?;
        let node = volume
            .nodes
            .get_mut(&self.inode)
            .ok_or(Error::InvalidData)?;
        if node.is_directory() {
            return Err(Error::IsDirectory);
        }
        if node.data.len() < end {
            node.data
                .try_reserve(end - node.data.len())
                .map_err(|_| Error::NoSpace)?;
            node.data.resize(end, 0);
        }
        node.data[offset..end].copy_from_slice(bytes);
        Ok(bytes.len())
    }

    pub fn truncate(&self, size: usize) -> Result<(), Error> {
        if size > MAX_FILE_SIZE {
            return Err(Error::FileTooLarge);
        }
        let mut volume = self.volume.borrow_mut();
        volume.check_growth(self.inode, size)?;
        let node = volume
            .nodes
            .get_mut(&self.inode)
            .ok_or(Error::InvalidData)?;
        if node.is_directory() {
            return Err(Error::IsDirectory);
        }
        if size > node.data.len() {
            node.data
                .try_reserve(size - node.data.len())
                .map_err(|_| Error::NoSpace)?;
        }
        node.data.resize(size, 0);
        Ok(())
    }

    pub fn fsync(&self) -> Result<(), Error> {
        Ok(())
    }
}

impl ReadAt for File {
    fn len(&self) -> usize {
        self.volume
            .borrow()
            .nodes
            .get(&self.inode)
            .map_or(0, |node| node.data.len())
    }

    fn read_at(&self, offset: usize, output: &mut [u8]) -> Result<usize, Error> {
        let volume = self.volume.borrow();
        let node = volume.nodes.get(&self.inode).ok_or(Error::InvalidData)?;
        if node.is_directory() {
            return Err(Error::IsDirectory);
        }
        let start = offset.min(node.data.len());
        let count = output.len().min(node.data.len() - start);
        output[..count].copy_from_slice(&node.data[start..start + count]);
        Ok(count)
    }
}

impl Volume {
    fn path(&self, inode: u64) -> Result<Vec<u8>, Error> {
        let mut parts = Vec::new();
        let mut current = inode;
        while current != ROOT {
            let node = self.nodes.get(&current).ok_or(Error::InvalidData)?;
            let parent = self.nodes.get(&node.parent).ok_or(Error::InvalidData)?;
            let name = parent
                .children
                .iter()
                .find_map(|(name, child)| (*child == current).then(|| name.clone()))
                .ok_or(Error::InvalidData)?;
            parts.push(name);
            current = node.parent;
        }
        parts.reverse();
        Ok(parts.join(&b'/'))
    }
}

impl Drop for File {
    fn drop(&mut self) {
        let mut volume = self.volume.borrow_mut();
        let closed = {
            let Some(count) = volume.open.get_mut(&self.inode) else {
                return;
            };
            *count = count.saturating_sub(1);
            *count == 0
        };
        if closed {
            volume.open.remove(&self.inode);
            if volume
                .nodes
                .get(&self.inode)
                .is_some_and(|node| node.links == 0)
            {
                volume.nodes.remove(&self.inode);
            }
        }
    }
}

impl Node {
    fn is_directory(&self) -> bool {
        self.mode & 0o170000 == 0o040000
    }

    fn is_symlink(&self) -> bool {
        self.mode & 0o170000 == 0o120000
    }

    fn file_type(&self) -> FileType {
        FileType::from_mode(self.mode)
    }
}
