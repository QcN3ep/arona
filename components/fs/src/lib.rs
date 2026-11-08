// SPDX-License-Identifier: MPL-2.0

//! 文件访问接口与文件服务。

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod archive;
pub mod block;
pub mod client;
mod control;
pub mod ext4;
mod metadata;
pub mod namespace;
mod open;
mod path;
mod protocol;
pub mod service;

pub use client::{Client, Directory, File};
pub use control::Control;
pub use metadata::{Attributes, Metadata, Timestamp};
pub use open::OpenOptions;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidRequest,
    InvalidData,
    NotFound,
    IsDirectory,
    NotDirectory,
    Unsupported,
    Io,
    Busy,
    ReadOnly,
    NoSpace,
    FileTooLarge,
    AlreadyExists,
    NotEmpty,
    CrossDevice,
    Device(devmgr::Error),
    UnexpectedEof,
    Unavailable,
    Runtime(runtime::Error),
    Call(runtime::ipc::CallError),
}

impl From<devmgr::Error> for Error {
    fn from(error: devmgr::Error) -> Self {
        Self::Device(error)
    }
}

impl From<runtime::Error> for Error {
    fn from(error: runtime::Error) -> Self {
        Self::Runtime(error)
    }
}

pub trait ReadAt {
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn read_at(&self, offset: usize, output: &mut [u8]) -> Result<usize, Error>;

    fn read_exact_at(&self, mut offset: usize, mut output: &mut [u8]) -> Result<(), Error> {
        while !output.is_empty() {
            let count = self.read_at(offset, output)?;
            if count == 0 {
                return Err(Error::UnexpectedEof);
            }
            if count > output.len() {
                return Err(Error::InvalidData);
            }
            offset = offset.checked_add(count).ok_or(Error::InvalidRequest)?;
            output = &mut output[count..];
        }
        Ok(())
    }
}

pub trait FileSystem {
    type File<'a>: ReadAt
    where
        Self: 'a;

    fn open(&self, path: &str) -> Result<Self::File<'_>, Error>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirEntry {
    pub name: alloc::vec::Vec<u8>,
    pub directory: bool,
}
