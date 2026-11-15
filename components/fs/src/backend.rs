// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Operations implemented by each mounted filesystem backend.
//!
//! A [`Backend`] is the mounted filesystem and a [`File`] is an open node. Both are used
//! through trait objects in the namespace, so a backend states each operation once and
//! operations it cannot provide default to [`Error::Unsupported`].

use crate::{Attributes, DirEntry, Error, Metadata, Statistics};
use alloc::{boxed::Box, vec::Vec};
use runtime::{FrameAllocation, Handle};

pub(crate) trait Backend {
    fn sync(&self) -> Result<(), Error> {
        Ok(())
    }

    fn statistics(&self) -> Result<Statistics, Error>;

    fn unmount(&self) -> Result<(), Error>;

    fn open_path(&self, path: &[u8]) -> Result<Box<dyn File>, Error> {
        self.open_bytes(path)
    }

    fn open_bytes(&self, path: &[u8]) -> Result<Box<dyn File>, Error>;

    fn open_directory(&self, path: &[u8]) -> Result<Box<dyn File>, Error>;

    fn create(&self, path: &[u8]) -> Result<Box<dyn File>, Error>;

    fn create_with_permissions(
        &self,
        path: &[u8],
        permissions: u16,
    ) -> Result<Box<dyn File>, Error>;

    fn metadata(&self, path: &[u8]) -> Result<Metadata, Error>;

    fn hard_link(&self, _source: &[u8], _target: &[u8]) -> Result<(), Error> {
        Err(Error::Unsupported)
    }

    fn symlink(&self, _target: &[u8], _path: &[u8]) -> Result<(), Error> {
        Err(Error::Unsupported)
    }

    fn read_link(&self, _path: &[u8]) -> Result<Vec<u8>, Error> {
        Err(Error::Unsupported)
    }

    fn mkdir_with_permissions(&self, _path: &[u8], _permissions: u16) -> Result<(), Error> {
        Err(Error::Unsupported)
    }

    fn unlink(&self, _path: &[u8]) -> Result<(), Error> {
        Err(Error::Unsupported)
    }

    fn rename(&self, _source: &[u8], _target: &[u8]) -> Result<(), Error> {
        Err(Error::Unsupported)
    }

    fn read_dir(&self, _path: &[u8]) -> Result<Vec<DirEntry>, Error> {
        Err(Error::Unsupported)
    }

    /// Capacity this filesystem reserves from the global in-memory volume budget.
    fn memory_limit(&self) -> usize {
        0
    }
}

pub(crate) trait File: crate::ReadAt {
    fn statistics(&self) -> Result<Statistics, Error>;

    fn path(&self) -> Result<Vec<u8>, Error>;

    fn check_writable(&self) -> Result<(), Error>;

    fn read_dir(&self) -> Result<Vec<DirEntry>, Error>;

    fn metadata(&self) -> Result<Metadata, Error>;

    fn parent_metadata(&self) -> Result<Option<Metadata>, Error>;

    fn set_attributes(&self, attributes: Attributes) -> Result<(), Error>;

    fn append(&self, bytes: &[u8]) -> Result<(usize, usize), Error>;

    fn fsync(&self) -> Result<(), Error>;

    fn write_at(&self, offset: usize, bytes: &[u8]) -> Result<usize, Error>;

    fn truncate(&self, size: usize) -> Result<(), Error>;

    /// Huge page size exposed by this file, or zero for ordinary files.
    fn hugepage_size(&self) -> u64 {
        0
    }

    /// Returns the huge page block that contains `offset`.
    fn map_huge(&self, _offset: usize) -> Result<Handle<FrameAllocation>, Error> {
        Err(Error::Unsupported)
    }
}
