// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! In-memory filesystem with byte-addressed file contents.

use crate::backend::{Backend, File as BackendFile};
use crate::memory::{Context, FileData, MemoryFile, MemoryVolume};
use crate::{Attributes, DirEntry, Error, Metadata, ReadAt, Statistics};
use alloc::boxed::Box;
use alloc::vec::Vec;

const BLOCK_SIZE: usize = 4096;
const DEFAULT_CAPACITY: usize = 64 * 1024 * 1024;

pub struct Tmpfs {
    inner: MemoryVolume<Bytes>,
}

pub struct File {
    inner: MemoryFile<Bytes>,
}

struct Bytes(Vec<u8>);

impl FileData for Bytes {
    const MAGIC: u64 = 0x0102_1994;
    type Config = ();
    const DIRECTORY_SIZE: u64 = 0;
    const SYMLINK_LIMIT: usize = 255;

    fn symlink_charge(_size: usize) -> usize {
        0
    }

    fn inode_counts(_linked: u64, retained: u64) -> (u64, u64) {
        const FREE: u64 = 1 << 20;
        (FREE + retained, FREE)
    }

    fn new((): ()) -> Self {
        Self(Vec::new())
    }
    fn size(&self) -> u64 {
        self.0.len() as u64
    }
    fn data_size(&self) -> usize {
        self.0.len().div_ceil(BLOCK_SIZE) * BLOCK_SIZE
    }
    fn blocks(&self) -> u64 {
        self.data_size() as u64 / 512
    }

    fn write(
        &mut self,
        context: &mut Context<'_>,
        offset: u64,
        input: &[u8],
    ) -> Result<usize, Error> {
        if input.is_empty() {
            return Ok(0);
        }
        let end = offset
            .checked_add(input.len() as u64)
            .filter(|end| *end <= crate::memory::MAX_FILE_SIZE)
            .ok_or(Error::FileTooLarge)? as usize;
        if end > self.0.len() {
            self.truncate(context, end as u64)?;
        }
        self.0[offset as usize..end].copy_from_slice(input);
        Ok(input.len())
    }

    fn truncate(&mut self, context: &mut Context<'_>, size: u64) -> Result<(), Error> {
        if size > crate::memory::MAX_FILE_SIZE {
            return Err(Error::FileTooLarge);
        }
        let size = size as usize;
        let before = self.data_size();
        let after = size.div_ceil(BLOCK_SIZE) * BLOCK_SIZE;
        if after > before {
            context.reserve(after - before)?;
        }
        if size > self.0.len() && self.0.try_reserve(size - self.0.len()).is_err() {
            context.release(after.saturating_sub(before));
            return Err(Error::NoSpace);
        }
        self.0.resize(size, 0);
        if after < before {
            context.release(before - after);
        }
        Ok(())
    }

    fn read(&self, offset: u64, output: &mut [u8]) -> Result<usize, Error> {
        let start = (offset as usize).min(self.0.len());
        let count = output.len().min(self.0.len() - start);
        output[..count].copy_from_slice(&self.0[start..start + count]);
        Ok(count)
    }
}

impl Tmpfs {
    pub fn new() -> Result<Self, Error> {
        Self::with_capacity(DEFAULT_CAPACITY)
    }
    pub fn with_capacity(capacity: usize) -> Result<Self, Error> {
        Ok(Self {
            inner: MemoryVolume::with_capacity((), capacity)?,
        })
    }
    pub(crate) fn limit(&self) -> usize {
        self.inner.limit()
    }
    pub fn statistics(&self) -> Result<Statistics, Error> {
        self.inner.statistics()
    }
    pub fn unmount(&self) -> Result<(), Error> {
        self.inner.unmount()
    }
    pub fn metadata(&self, path: &[u8]) -> Result<Metadata, Error> {
        self.inner.metadata(path)
    }
    pub fn read_dir(&self, path: &[u8]) -> Result<Vec<DirEntry>, Error> {
        self.inner.read_dir(path)
    }
    pub fn mkdir(&self, path: &[u8]) -> Result<(), Error> {
        self.mkdir_with_permissions(path, 0o755)
    }
    pub fn mkdir_with_permissions(&self, path: &[u8], permissions: u16) -> Result<(), Error> {
        self.inner.mkdir_with_permissions(path, permissions)
    }
    pub fn unlink(&self, path: &[u8]) -> Result<(), Error> {
        self.inner.unlink(path)
    }
    pub fn rename(&self, source: &[u8], target: &[u8]) -> Result<(), Error> {
        self.inner.rename(source, target)
    }
    pub fn hard_link(&self, source: &[u8], target: &[u8]) -> Result<(), Error> {
        self.inner.hard_link(source, target)
    }
    pub fn symlink(&self, target: &[u8], path: &[u8]) -> Result<(), Error> {
        self.inner.symlink(target, path)
    }
    pub fn read_link(&self, path: &[u8]) -> Result<Vec<u8>, Error> {
        self.inner.read_link(path)
    }
    pub fn open_bytes(&self, path: &[u8]) -> Result<File, Error> {
        Ok(File {
            inner: self.inner.open_node(path, false, false)?,
        })
    }
    pub fn open_directory(&self, path: &[u8]) -> Result<File, Error> {
        Ok(File {
            inner: self.inner.open_node(path, true, false)?,
        })
    }
    pub fn open_path(&self, path: &[u8]) -> Result<File, Error> {
        Ok(File {
            inner: self.inner.open_node(path, false, true)?,
        })
    }
    pub fn create(&self, path: &[u8]) -> Result<File, Error> {
        self.create_with_permissions(path, 0o644)
    }
    pub fn create_with_permissions(&self, path: &[u8], permissions: u16) -> Result<File, Error> {
        Ok(File {
            inner: self.inner.create_file(path, permissions)?,
        })
    }
}

impl File {
    pub fn statistics(&self) -> Result<Statistics, Error> {
        self.inner.statistics()
    }
    pub fn path(&self) -> Result<Vec<u8>, Error> {
        self.inner.path()
    }
    pub fn check_writable(&self) -> Result<(), Error> {
        self.inner.check_writable()
    }
    pub fn read_dir(&self) -> Result<Vec<DirEntry>, Error> {
        self.inner.read_dir()
    }
    pub fn metadata(&self) -> Result<Metadata, Error> {
        self.inner.metadata()
    }
    pub fn parent_metadata(&self) -> Result<Option<Metadata>, Error> {
        self.inner.parent_metadata()
    }
    pub fn set_attributes(&self, attributes: Attributes) -> Result<(), Error> {
        self.inner.set_attributes(attributes)
    }
    pub fn append(&self, bytes: &[u8]) -> Result<(usize, usize), Error> {
        self.inner.append(bytes)
    }
    pub fn fsync(&self) -> Result<(), Error> {
        self.inner.fsync()
    }
    pub fn write_at(&self, offset: usize, bytes: &[u8]) -> Result<usize, Error> {
        self.inner.write_at(offset, bytes)
    }
    pub fn truncate(&self, size: usize) -> Result<(), Error> {
        self.inner.truncate(size)
    }
}
impl ReadAt for File {
    fn len(&self) -> usize {
        self.inner.len()
    }
    fn read_at(&self, offset: usize, output: &mut [u8]) -> Result<usize, Error> {
        self.inner.read_at(offset, output)
    }
}

impl crate::backend::Backend for Tmpfs {
    fn open_path(&self, path: &[u8]) -> Result<Box<dyn crate::backend::File>, Error> {
        Tmpfs::open_path(self, path).map(|file| Box::new(file) as Box<dyn crate::backend::File>)
    }

    fn statistics(&self) -> Result<crate::Statistics, Error> {
        Tmpfs::statistics(self)
    }

    fn memory_limit(&self) -> usize {
        self.limit()
    }

    fn unmount(&self) -> Result<(), Error> {
        Tmpfs::unmount(self)
    }

    fn open_bytes(&self, path: &[u8]) -> Result<Box<dyn crate::backend::File>, Error> {
        Tmpfs::open_bytes(self, path).map(|file| Box::new(file) as Box<dyn crate::backend::File>)
    }

    fn open_directory(&self, path: &[u8]) -> Result<Box<dyn crate::backend::File>, Error> {
        Tmpfs::open_directory(self, path)
            .map(|file| Box::new(file) as Box<dyn crate::backend::File>)
    }

    fn create(&self, path: &[u8]) -> Result<Box<dyn crate::backend::File>, Error> {
        Tmpfs::create(self, path).map(|file| Box::new(file) as Box<dyn crate::backend::File>)
    }

    fn create_with_permissions(
        &self,
        path: &[u8],
        permissions: u16,
    ) -> Result<Box<dyn crate::backend::File>, Error> {
        Tmpfs::create_with_permissions(self, path, permissions)
            .map(|file| Box::new(file) as Box<dyn crate::backend::File>)
    }

    fn metadata(&self, path: &[u8]) -> Result<crate::Metadata, Error> {
        Tmpfs::metadata(self, path)
    }

    fn hard_link(&self, source: &[u8], target: &[u8]) -> Result<(), Error> {
        Tmpfs::hard_link(self, source, target)
    }

    fn symlink(&self, target: &[u8], path: &[u8]) -> Result<(), Error> {
        Tmpfs::symlink(self, target, path)
    }

    fn read_link(&self, path: &[u8]) -> Result<Vec<u8>, Error> {
        Tmpfs::read_link(self, path)
    }

    fn mkdir_with_permissions(&self, path: &[u8], permissions: u16) -> Result<(), Error> {
        Tmpfs::mkdir_with_permissions(self, path, permissions)
    }

    fn unlink(&self, path: &[u8]) -> Result<(), Error> {
        Tmpfs::unlink(self, path)
    }

    fn rename(&self, source: &[u8], target: &[u8]) -> Result<(), Error> {
        Tmpfs::rename(self, source, target)
    }

    fn read_dir(&self, path: &[u8]) -> Result<Vec<crate::DirEntry>, Error> {
        Tmpfs::read_dir(self, path)
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
