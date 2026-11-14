// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Shared in-memory volume used by tmpfs and huge page files.
//!
//! The volume owns the inode map, directory entries, metadata, link and session
//! accounting. A backend supplies file contents through [`FileData`], which also decides
//! how bytes are charged against the volume limit and whether blocks can be mapped.

use crate::{Error, ReadAt, Statistics, Timestamp};
use alloc::{collections::BTreeMap, rc::Rc, vec::Vec};
use core::cell::RefCell;
use runtime::{FrameAllocation, Handle};

pub(crate) const BLOCK_SIZE: u64 = 4096;
pub(crate) const MAX_FILE_SIZE: u64 = 2 * 1024 * 1024 * 1024;
const DEFAULT_LIMIT: usize = 64 * 1024 * 1024;
const NAME_LENGTH: u64 = 255;
const ROOT_INODE: u64 = 1;

type InodeRef<F> = Rc<RefCell<Inode<F>>>;

/// Allocation state available to [`FileData`] operations.
pub(crate) struct Context<'a> {
    pub used: &'a mut usize,
    pub limit: usize,
    retired: &'a mut Vec<Retired>,
}

/// A block that a client may still reference after its file stopped accounting for it.
struct Retired {
    frames: Handle<FrameAllocation>,
    bytes: usize,
}

impl Context<'_> {
    pub(crate) fn reserve(&mut self, bytes: usize) -> Result<(), Error> {
        self.reclaim();
        if self
            .used
            .checked_add(bytes)
            .is_none_or(|used| used > self.limit)
        {
            return Err(Error::NoSpace);
        }
        *self.used += bytes;
        Ok(())
    }

    pub(crate) fn release(&mut self, bytes: usize) {
        *self.used = self.used.saturating_sub(bytes);
    }

    /// Keeps a block charged after its file dropped it while a client may still map it.
    pub(crate) fn retire(&mut self, frames: Handle<FrameAllocation>, bytes: usize) {
        self.retired.push(Retired { frames, bytes });
    }

    /// Releases retired blocks after the last client reference and mapping are gone.
    fn reclaim(&mut self) {
        let mut index = 0;
        while index < self.retired.len() {
            let state = self.retired[index].frames.state();
            // The caller's handle and the transient reference used by the query keep
            // two handles alive even when no client still owns the block.
            if state.is_ok_and(|state| state.handles <= 2 && state.mappings == 0) {
                let retired = self.retired.swap_remove(index);
                self.release(retired.bytes);
            } else {
                index += 1;
            }
        }
    }
}

/// File contents of an in-memory volume.
pub(crate) trait FileData: Sized {
    const MAGIC: u64;
    type Config: Copy;

    fn new(config: Self::Config) -> Self;

    fn size(&self) -> u64;

    /// Bytes charged against the volume limit.
    fn data_size(&self) -> usize;

    fn write(
        &mut self,
        context: &mut Context<'_>,
        offset: u64,
        input: &[u8],
    ) -> Result<usize, Error>;

    fn truncate(&mut self, context: &mut Context<'_>, size: u64) -> Result<(), Error>;

    /// Drops the contents after the last name and session are gone.
    ///
    /// A backend that handed frames to a client must retire blocks that may still be
    /// mapped instead of refunding them immediately.
    fn release(&mut self, context: &mut Context<'_>) {
        context.release(self.data_size());
    }

    fn read(&self, offset: u64, output: &mut [u8]) -> Result<usize, Error>;

    /// Materializes the block that contains `offset` and returns a handle for mapping it.
    fn map(
        &mut self,
        _context: &mut Context<'_>,
        _offset: u64,
    ) -> Result<Handle<FrameAllocation>, Error> {
        Err(Error::Unsupported)
    }

    /// Huge page size exposed to the file client, or zero for byte-addressed files.
    fn hugepage_size() -> u64 {
        0
    }
}

pub(crate) struct MemoryVolume<F: FileData> {
    volume: Rc<RefCell<Volume<F>>>,
}

pub(crate) struct MemoryFile<F: FileData> {
    volume: Rc<RefCell<Volume<F>>>,
    inode: InodeRef<F>,
}

struct Volume<F: FileData> {
    device: u64,
    mount: u64,
    inodes: BTreeMap<u64, InodeRef<F>>,
    next_inode: u64,
    used: usize,
    limit: usize,
    config: F::Config,
    retired: Vec<Retired>,
}

struct Inode<F: FileData> {
    number: u64,
    content: Content<F>,
    permissions: u16,
    uid: u32,
    gid: u32,
    links: u32,
    sessions: u32,
    accessed: Timestamp,
    modified: Timestamp,
    changed: Timestamp,
    birth: Timestamp,
}

enum Content<F: FileData> {
    File(F),
    Directory(BTreeMap<Vec<u8>, u64>),
    Symlink(Vec<u8>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    File,
    Directory,
    Symlink,
}

impl<F: FileData> Content<F> {
    fn kind(&self) -> Kind {
        match self {
            Self::File(_) => Kind::File,
            Self::Directory(_) => Kind::Directory,
            Self::Symlink(_) => Kind::Symlink,
        }
    }

    fn size(&self) -> u64 {
        match self {
            Self::File(file) => file.size(),
            Self::Symlink(target) => target.len() as u64,
            Self::Directory(_) => BLOCK_SIZE,
        }
    }
}

impl<F: FileData> Inode<F> {
    fn new(number: u64, content: Content<F>, permissions: u16, links: u32) -> Self {
        Self {
            number,
            content,
            permissions,
            uid: 0,
            gid: 0,
            links,
            sessions: 0,
            accessed: Timestamp::default(),
            modified: Timestamp::default(),
            changed: Timestamp::default(),
            birth: Timestamp::default(),
        }
    }

    fn kind(&self) -> Kind {
        self.content.kind()
    }

    fn metadata(&self, device: u64, mount: u64) -> crate::Metadata {
        let size = self.content.size();
        crate::Metadata {
            device,
            rdev: 0,
            mount,
            inode: self.number,
            size,
            blocks: size.div_ceil(512),
            links: self.links,
            mode: file_type(self.kind()) | u32::from(self.permissions),
            uid: self.uid,
            gid: self.gid,
            accessed: self.accessed,
            modified: self.modified,
            changed: self.changed,
            birth: self.birth,
        }
    }
}

impl<F: FileData> MemoryVolume<F> {
    /// Volume capacity counted against the namespace's global memory budget.
    pub(crate) fn limit(&self) -> usize {
        self.volume.borrow().limit
    }

    pub(crate) fn new(config: F::Config) -> Result<Self, Error> {
        let device = crate::metadata::device_identity()?;
        let mount = crate::metadata::mount_identity()?;
        let mut inodes = BTreeMap::new();
        inodes.insert(
            ROOT_INODE,
            Rc::new(RefCell::new(Inode::new(
                ROOT_INODE,
                Content::Directory(BTreeMap::new()),
                0o755,
                2,
            ))),
        );
        Ok(Self {
            volume: Rc::new(RefCell::new(Volume {
                device,
                mount,
                inodes,
                next_inode: ROOT_INODE + 1,
                used: 0,
                limit: DEFAULT_LIMIT,
                config,
                retired: Vec::new(),
            })),
        })
    }

    fn open_node(&self, path: &[u8], directory: bool) -> Result<MemoryFile<F>, Error> {
        let volume = self.volume.borrow();
        let number = volume.lookup(path)?;
        let inode = volume.inode(number)?;
        let kind = inode.borrow().kind();
        match (kind, directory) {
            (Kind::Directory, true) | (Kind::File, false) => {}
            (Kind::Directory, false) => return Err(Error::IsDirectory),
            (Kind::File | Kind::Symlink, true) => return Err(Error::NotDirectory),
            (Kind::Symlink, false) => return Err(Error::InvalidRequest),
        }
        {
            let mut inode = inode.borrow_mut();
            inode.sessions = inode.sessions.checked_add(1).ok_or(Error::NoSpace)?;
        }
        Ok(MemoryFile {
            volume: Rc::clone(&self.volume),
            inode,
        })
    }
}

impl<F: FileData> Volume<F> {
    fn create_node(
        &mut self,
        path: &[u8],
        directory: bool,
        permissions: u16,
    ) -> Result<u64, Error> {
        let (parent_number, name) = self.parent(path)?;
        let parent = self.inode(parent_number)?;
        self.check_absent(&parent, &name)?;
        let number = self.next_inode;
        self.next_inode = number.checked_add(1).ok_or(Error::NoSpace)?;
        let content = if directory {
            Content::Directory(BTreeMap::new())
        } else {
            Content::File(F::new(self.config))
        };
        let links = if directory { 2 } else { 1 };
        self.inodes.insert(
            number,
            Rc::new(RefCell::new(Inode::new(
                number,
                content,
                permissions,
                links,
            ))),
        );
        let mut parent = parent.borrow_mut();
        let Content::Directory(entries) = &mut parent.content else {
            return Err(Error::NotDirectory);
        };
        entries.insert(name, number);
        if directory {
            parent.links = parent.links.checked_add(1).ok_or(Error::NoSpace)?;
        }
        Ok(number)
    }

    fn check_absent(&self, parent: &InodeRef<F>, name: &[u8]) -> Result<(), Error> {
        let parent = parent.borrow();
        let Content::Directory(entries) = &parent.content else {
            return Err(Error::NotDirectory);
        };
        if entries.contains_key(name) {
            return Err(Error::AlreadyExists);
        }
        Ok(())
    }

    fn unlink_path(&mut self, path: &[u8]) -> Result<(), Error> {
        let (parent_number, name) = self.parent(path)?;
        let parent = self.inode(parent_number)?;
        let number = {
            let parent = parent.borrow();
            let Content::Directory(entries) = &parent.content else {
                return Err(Error::NotDirectory);
            };
            *entries.get(&name).ok_or(Error::NotFound)?
        };
        let inode = self.inode(number)?;
        self.check_removable(&inode)?;
        let mut parent = parent.borrow_mut();
        let Content::Directory(entries) = &mut parent.content else {
            return Err(Error::NotDirectory);
        };
        entries.remove(&name);
        if inode.borrow().kind() == Kind::Directory {
            parent.links = parent.links.checked_sub(1).ok_or(Error::InvalidData)?;
        }
        drop(parent);
        if inode.borrow().kind() == Kind::Directory {
            self.remove_inode(&inode);
        } else {
            self.remove_link(&inode);
        }
        Ok(())
    }

    fn check_removable(&self, inode: &InodeRef<F>) -> Result<(), Error> {
        let inode = inode.borrow();
        let Content::Directory(entries) = &inode.content else {
            return Ok(());
        };
        if entries.is_empty() {
            Ok(())
        } else {
            Err(Error::NotEmpty)
        }
    }

    fn rename_path(&mut self, source: &[u8], target: &[u8]) -> Result<(), Error> {
        let source_path = crate::path::normalize(source)?;
        let target_path = crate::path::normalize(target)?;
        if source_path == target_path {
            return Ok(());
        }
        let (old_parent_number, old_name) = self.parent(source)?;
        let (new_parent_number, new_name) = self.parent(target)?;
        let old_parent = self.inode(old_parent_number)?;
        let new_parent = self.inode(new_parent_number)?;
        let number = {
            let old_parent = old_parent.borrow();
            let Content::Directory(entries) = &old_parent.content else {
                return Err(Error::NotDirectory);
            };
            *entries.get(&old_name).ok_or(Error::NotFound)?
        };
        let inode = self.inode(number)?;
        let directory = inode.borrow().kind() == Kind::Directory;
        if directory
            && target_path.starts_with(&source_path)
            && target_path.get(source_path.len()) == Some(&b'/')
        {
            return Err(Error::InvalidRequest);
        }
        let replacement = {
            let new_parent = new_parent.borrow();
            let Content::Directory(entries) = &new_parent.content else {
                return Err(Error::NotDirectory);
            };
            entries.get(&new_name).copied()
        };
        if replacement == Some(number) {
            return Ok(());
        }
        let replaced = replacement.map(|other| self.inode(other)).transpose()?;
        let replaced_directory = replaced
            .as_ref()
            .is_some_and(|inode| inode.borrow().kind() == Kind::Directory);
        if let Some(replaced) = &replaced {
            if replaced_directory != directory {
                return Err(if directory {
                    Error::IsDirectory
                } else {
                    Error::NotDirectory
                });
            }
            self.check_removable(replaced)?;
        }
        let old_links = old_parent.borrow().links;
        let new_links = new_parent.borrow().links;
        if old_parent_number == new_parent_number {
            let links = new_links
                .checked_sub(u32::from(replaced_directory))
                .ok_or(Error::InvalidData)?;
            let mut parent = old_parent.borrow_mut();
            {
                let Content::Directory(entries) = &mut parent.content else {
                    return Err(Error::NotDirectory);
                };
                entries.remove(&old_name);
                entries.remove(&new_name);
                entries.insert(new_name.clone(), number);
            }
            parent.links = links;
        } else {
            let old_links = old_links
                .checked_sub(u32::from(directory))
                .ok_or(Error::InvalidData)?;
            let new_links = new_links
                .checked_add(u32::from(directory))
                .and_then(|links| links.checked_sub(u32::from(replaced_directory)))
                .ok_or(Error::NoSpace)?;
            {
                let mut parent = old_parent.borrow_mut();
                let Content::Directory(entries) = &mut parent.content else {
                    return Err(Error::NotDirectory);
                };
                entries.remove(&old_name);
                parent.links = old_links;
            }
            let mut parent = new_parent.borrow_mut();
            {
                let Content::Directory(entries) = &mut parent.content else {
                    return Err(Error::NotDirectory);
                };
                entries.remove(&new_name);
                entries.insert(new_name, number);
            }
            parent.links = new_links;
        }
        if let Some(replaced) = replaced {
            self.remove_inode_or_link(&replaced);
        }
        Ok(())
    }

    fn remove_inode_or_link(&mut self, inode: &InodeRef<F>) {
        if inode.borrow().kind() == Kind::Directory {
            self.remove_inode(inode);
        } else {
            self.remove_link(inode);
        }
    }

    fn remove_link(&mut self, inode: &InodeRef<F>) {
        let (number, orphaned) = {
            let mut inode = inode.borrow_mut();
            inode.links = inode.links.saturating_sub(1);
            if inode.links != 0 {
                return;
            }
            (inode.number, inode.sessions == 0)
        };
        self.inodes.remove(&number);
        if orphaned {
            self.release(inode);
        }
    }

    /// Releases the data of an unlinked inode after its last session closes.
    fn close(&mut self, inode: &InodeRef<F>) {
        let orphaned = {
            let mut inode = inode.borrow_mut();
            inode.sessions = inode.sessions.saturating_sub(1);
            inode.links == 0 && inode.sessions == 0
        };
        if orphaned {
            self.release(inode);
        }
    }

    fn remove_inode(&mut self, inode: &InodeRef<F>) {
        let number = inode.borrow().number;
        self.inodes.remove(&number);
        self.release(inode);
    }

    fn release(&mut self, inode: &InodeRef<F>) {
        let mut context = self.context();
        let mut inode = inode.borrow_mut();
        let bytes = match &mut inode.content {
            Content::File(file) => {
                file.release(&mut context);
                0
            }
            Content::Symlink(target) => target.len(),
            Content::Directory(_) => 0,
        };
        context.release(bytes);
    }

    fn hard_link_path(&mut self, source: &[u8], target: &[u8]) -> Result<(), Error> {
        let number = self.lookup(source)?;
        let inode = self.inode(number)?;
        if inode.borrow().kind() == Kind::Directory {
            return Err(Error::IsDirectory);
        }
        let (parent_number, name) = self.parent(target)?;
        let parent = self.inode(parent_number)?;
        self.check_absent(&parent, &name)?;
        {
            let mut inode = inode.borrow_mut();
            inode.links = inode.links.checked_add(1).ok_or(Error::NoSpace)?;
        }
        let mut parent = parent.borrow_mut();
        let Content::Directory(entries) = &mut parent.content else {
            return Err(Error::NotDirectory);
        };
        entries.insert(name, number);
        Ok(())
    }

    fn symlink_path(&mut self, target: &[u8], path: &[u8]) -> Result<(), Error> {
        if target.is_empty() || target.len() >= BLOCK_SIZE as usize || target.contains(&0) {
            return Err(Error::InvalidRequest);
        }
        if self
            .used
            .checked_add(target.len())
            .is_none_or(|used| used > self.limit)
        {
            return Err(Error::NoSpace);
        }
        let (parent_number, name) = self.parent(path)?;
        let parent = self.inode(parent_number)?;
        self.check_absent(&parent, &name)?;
        let number = self.next_inode;
        self.next_inode = number.checked_add(1).ok_or(Error::NoSpace)?;
        self.inodes.insert(
            number,
            Rc::new(RefCell::new(Inode::new(
                number,
                Content::Symlink(target.to_vec()),
                0o777,
                1,
            ))),
        );
        self.used += target.len();
        let mut parent = parent.borrow_mut();
        let Content::Directory(entries) = &mut parent.content else {
            return Err(Error::NotDirectory);
        };
        entries.insert(name, number);
        Ok(())
    }

    fn read_link_path(&self, path: &[u8]) -> Result<Vec<u8>, Error> {
        let number = self.lookup(path)?;
        let inode = self.inode(number)?;
        let inode = inode.borrow();
        let Content::Symlink(target) = &inode.content else {
            return Err(Error::InvalidRequest);
        };
        Ok(target.clone())
    }

    fn read_dir_path(&self, path: &[u8]) -> Result<Vec<crate::DirEntry>, Error> {
        let number = self.lookup(path)?;
        let inode = self.inode(number)?;
        let inode = inode.borrow();
        let Content::Directory(entries) = &inode.content else {
            return Err(Error::NotDirectory);
        };
        let mut result = Vec::with_capacity(entries.len());
        for (name, number) in entries {
            result.push(crate::DirEntry {
                name: name.clone(),
                inode: *number,
                kind: entry_type(self.kind(*number)?),
            });
        }
        Ok(result)
    }

    fn set_attributes_of(
        &mut self,
        inode: &InodeRef<F>,
        attributes: crate::Attributes,
    ) -> Result<(), Error> {
        attributes.validate()?;
        let mut inode = inode.borrow_mut();
        inode.permissions = attributes.permissions as u16;
        inode.uid = attributes.uid;
        inode.gid = attributes.gid;
        inode.accessed = attributes.accessed;
        inode.modified = attributes.modified;
        inode.changed = attributes.changed;
        Ok(())
    }

    fn write(&mut self, inode: &InodeRef<F>, offset: usize, input: &[u8]) -> Result<usize, Error> {
        let mut inode = inode.borrow_mut();
        let Content::File(file) = &mut inode.content else {
            return Err(Error::InvalidRequest);
        };
        let mut context = self.context();
        file.write(&mut context, offset as u64, input)
    }

    fn truncate_inode(&mut self, inode: &InodeRef<F>, size: usize) -> Result<(), Error> {
        if size as u64 > MAX_FILE_SIZE {
            return Err(Error::FileTooLarge);
        }
        let mut inode = inode.borrow_mut();
        let Content::File(file) = &mut inode.content else {
            return Err(Error::InvalidRequest);
        };
        let mut context = self.context();
        file.truncate(&mut context, size as u64)
    }

    fn read_inode(
        &self,
        inode: &InodeRef<F>,
        offset: usize,
        output: &mut [u8],
    ) -> Result<usize, Error> {
        let inode = inode.borrow();
        let Content::File(file) = &inode.content else {
            return Err(Error::InvalidRequest);
        };
        file.read(offset as u64, output)
    }

    fn map_page(
        &mut self,
        inode: &InodeRef<F>,
        offset: usize,
    ) -> Result<Handle<FrameAllocation>, Error> {
        let mut inode = inode.borrow_mut();
        let Content::File(file) = &mut inode.content else {
            return Err(Error::InvalidRequest);
        };
        let mut context = self.context();
        file.map(&mut context, offset as u64)
    }

    fn context(&mut self) -> Context<'_> {
        self.reclaim_retired();
        Context {
            used: &mut self.used,
            limit: self.limit,
            retired: &mut self.retired,
        }
    }

    fn reclaim_retired(&mut self) {
        let mut context = Context {
            used: &mut self.used,
            limit: self.limit,
            retired: &mut self.retired,
        };
        context.reclaim();
    }

    fn statistics_value(&self) -> Statistics {
        let blocks = (self.limit as u64).div_ceil(BLOCK_SIZE);
        let free = self.limit.saturating_sub(self.used) as u64 / BLOCK_SIZE;
        let files = (1 << 20) as u64;
        Statistics {
            kind: F::MAGIC,
            device: self.device,
            block_size: BLOCK_SIZE,
            blocks,
            free_blocks: free,
            available_blocks: free,
            files,
            free_files: files.saturating_sub(self.inodes.len() as u64),
            name_length: NAME_LENGTH,
            flags: 0,
        }
    }

    fn inode(&self, number: u64) -> Result<InodeRef<F>, Error> {
        self.inodes.get(&number).cloned().ok_or(Error::NotFound)
    }

    fn kind(&self, number: u64) -> Result<Kind, Error> {
        Ok(self.inode(number)?.borrow().kind())
    }

    fn metadata_of(&self, number: u64) -> Result<crate::Metadata, Error> {
        Ok(self
            .inode(number)?
            .borrow()
            .metadata(self.device, self.mount))
    }

    fn lookup(&self, path: &[u8]) -> Result<u64, Error> {
        let mut number = ROOT_INODE;
        for name in crate::path::components(path)? {
            let inode = self.inode(number)?;
            let inode = inode.borrow();
            let Content::Directory(entries) = &inode.content else {
                return Err(Error::NotDirectory);
            };
            number = *entries.get(name).ok_or(Error::NotFound)?;
        }
        Ok(number)
    }

    fn parent(&self, path: &[u8]) -> Result<(u64, Vec<u8>), Error> {
        let parts = crate::path::components(path)?;
        let (name, parents) = parts.split_last().ok_or(Error::InvalidRequest)?;
        let mut number = ROOT_INODE;
        for part in parents {
            let inode = self.inode(number)?;
            let inode = inode.borrow();
            let Content::Directory(entries) = &inode.content else {
                return Err(Error::NotDirectory);
            };
            number = *entries.get(*part).ok_or(Error::NotFound)?;
        }
        if self.kind(number)? != Kind::Directory {
            return Err(Error::NotDirectory);
        }
        Ok((number, name.to_vec()))
    }

    fn parent_of(&self, number: u64) -> Result<Option<u64>, Error> {
        if number == ROOT_INODE {
            return Ok(None);
        }
        for (parent, inode) in &self.inodes {
            let inode = inode.borrow();
            let Content::Directory(entries) = &inode.content else {
                continue;
            };
            if entries.values().any(|child| *child == number) {
                return Ok(Some(*parent));
            }
        }
        Err(Error::NotFound)
    }

    fn path_of(&self, number: u64) -> Result<Vec<u8>, Error> {
        if number == ROOT_INODE {
            return Ok(Vec::new());
        }
        let mut pending = alloc::vec![(ROOT_INODE, Vec::new())];
        while let Some((parent, prefix)) = pending.pop() {
            let inode = self.inode(parent)?;
            let inode = inode.borrow();
            let Content::Directory(entries) = &inode.content else {
                continue;
            };
            for (name, child) in entries {
                let mut path = prefix.clone();
                if !path.is_empty() {
                    path.push(b'/');
                }
                path.extend_from_slice(name);
                if *child == number {
                    return Ok(path);
                }
                if self.kind(*child)? == Kind::Directory {
                    pending.push((*child, path));
                }
            }
        }
        Err(Error::NotFound)
    }
}

impl<F: FileData + 'static> crate::backend::Backend for MemoryVolume<F> {
    fn memory_limit(&self) -> usize {
        self.limit()
    }

    fn statistics(&self) -> Result<Statistics, Error> {
        let mut volume = self.volume.borrow_mut();
        volume.reclaim_retired();
        Ok(volume.statistics_value())
    }

    fn unmount(&self) -> Result<(), Error> {
        Ok(())
    }

    fn open_bytes(
        &self,
        path: &[u8],
    ) -> Result<alloc::boxed::Box<dyn crate::backend::File>, Error> {
        let file = self.open_node(path, false)?;
        Ok(alloc::boxed::Box::new(file))
    }

    fn open_directory(
        &self,
        path: &[u8],
    ) -> Result<alloc::boxed::Box<dyn crate::backend::File>, Error> {
        let file = self.open_node(path, true)?;
        Ok(alloc::boxed::Box::new(file))
    }

    fn create(&self, path: &[u8]) -> Result<alloc::boxed::Box<dyn crate::backend::File>, Error> {
        self.create_with_permissions(path, 0o644)
    }

    fn create_with_permissions(
        &self,
        path: &[u8],
        permissions: u16,
    ) -> Result<alloc::boxed::Box<dyn crate::backend::File>, Error> {
        validate_permissions(permissions)?;
        self.volume
            .borrow_mut()
            .create_node(path, false, permissions)?;
        self.open_bytes(path)
    }

    fn metadata(&self, path: &[u8]) -> Result<crate::Metadata, Error> {
        let volume = self.volume.borrow();
        let number = volume.lookup(path)?;
        volume.metadata_of(number)
    }

    fn hard_link(&self, source: &[u8], target: &[u8]) -> Result<(), Error> {
        self.volume.borrow_mut().hard_link_path(source, target)
    }

    fn symlink(&self, target: &[u8], path: &[u8]) -> Result<(), Error> {
        self.volume.borrow_mut().symlink_path(target, path)
    }

    fn read_link(&self, path: &[u8]) -> Result<Vec<u8>, Error> {
        self.volume.borrow().read_link_path(path)
    }

    fn mkdir_with_permissions(&self, path: &[u8], permissions: u16) -> Result<(), Error> {
        validate_permissions(permissions)?;
        self.volume
            .borrow_mut()
            .create_node(path, true, permissions)
            .map(|_| ())
    }

    fn unlink(&self, path: &[u8]) -> Result<(), Error> {
        self.volume.borrow_mut().unlink_path(path)
    }

    fn rename(&self, source: &[u8], target: &[u8]) -> Result<(), Error> {
        self.volume.borrow_mut().rename_path(source, target)
    }

    fn read_dir(&self, path: &[u8]) -> Result<Vec<crate::DirEntry>, Error> {
        self.volume.borrow().read_dir_path(path)
    }
}

impl<F: FileData> MemoryFile<F> {
    fn kind(&self) -> Result<Kind, Error> {
        Ok(self.inode.borrow().kind())
    }
}

impl<F: FileData + 'static> crate::backend::File for MemoryFile<F> {
    fn statistics(&self) -> Result<Statistics, Error> {
        let mut volume = self.volume.borrow_mut();
        volume.reclaim_retired();
        Ok(volume.statistics_value())
    }

    fn path(&self) -> Result<Vec<u8>, Error> {
        let number = self.inode.borrow().number;
        self.volume.borrow().path_of(number)
    }

    fn check_writable(&self) -> Result<(), Error> {
        Ok(())
    }

    fn read_dir(&self) -> Result<Vec<crate::DirEntry>, Error> {
        let volume = self.volume.borrow();
        let inode = self.inode.borrow();
        let Content::Directory(entries) = &inode.content else {
            return Err(Error::NotDirectory);
        };
        let mut result = Vec::with_capacity(entries.len());
        for (name, number) in entries {
            result.push(crate::DirEntry {
                name: name.clone(),
                inode: *number,
                kind: entry_type(volume.kind(*number)?),
            });
        }
        Ok(result)
    }

    fn metadata(&self) -> Result<crate::Metadata, Error> {
        let volume = self.volume.borrow();
        Ok(self.inode.borrow().metadata(volume.device, volume.mount))
    }

    fn parent_metadata(&self) -> Result<Option<crate::Metadata>, Error> {
        let number = self.inode.borrow().number;
        let volume = self.volume.borrow();
        volume
            .parent_of(number)?
            .map(|parent| volume.metadata_of(parent))
            .transpose()
    }

    fn set_attributes(&self, attributes: crate::Attributes) -> Result<(), Error> {
        self.volume
            .borrow_mut()
            .set_attributes_of(&self.inode, attributes)
    }

    fn append(&self, input: &[u8]) -> Result<(usize, usize), Error> {
        let offset = {
            let inode = self.inode.borrow();
            usize::try_from(inode.content.size()).map_err(|_| Error::FileTooLarge)?
        };
        let count = self.volume.borrow_mut().write(&self.inode, offset, input)?;
        Ok((offset, count))
    }

    fn fsync(&self) -> Result<(), Error> {
        Ok(())
    }

    fn write_at(&self, offset: usize, input: &[u8]) -> Result<usize, Error> {
        self.volume.borrow_mut().write(&self.inode, offset, input)
    }

    fn truncate(&self, size: usize) -> Result<(), Error> {
        self.volume.borrow_mut().truncate_inode(&self.inode, size)
    }

    fn hugepage_size(&self) -> u64 {
        F::hugepage_size()
    }

    fn map_huge(&self, offset: usize) -> Result<Handle<FrameAllocation>, Error> {
        self.volume.borrow_mut().map_page(&self.inode, offset)
    }
}

impl<F: FileData> Drop for MemoryFile<F> {
    fn drop(&mut self) {
        self.volume.borrow_mut().close(&self.inode);
    }
}

impl<F: FileData> ReadAt for MemoryFile<F> {
    fn len(&self) -> usize {
        usize::try_from(self.inode.borrow().content.size()).unwrap_or(usize::MAX)
    }

    fn read_at(&self, offset: usize, output: &mut [u8]) -> Result<usize, Error> {
        if self.kind()? != Kind::File {
            return Err(Error::IsDirectory);
        }
        self.volume.borrow().read_inode(&self.inode, offset, output)
    }
}

fn validate_permissions(permissions: u16) -> Result<(), Error> {
    if permissions & !0o7777 != 0 {
        Err(Error::InvalidRequest)
    } else {
        Ok(())
    }
}

fn file_type(kind: Kind) -> u32 {
    match kind {
        Kind::File => 0o100000,
        Kind::Directory => 0o040000,
        Kind::Symlink => 0o120000,
    }
}

fn entry_type(kind: Kind) -> crate::FileType {
    match kind {
        Kind::File => crate::FileType::Regular,
        Kind::Directory => crate::FileType::Directory,
        Kind::Symlink => crate::FileType::SymbolicLink,
    }
}
