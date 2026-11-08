// SPDX-License-Identifier: MPL-2.0

//! Linux 程序的装载、系统调用转发与生命周期管理。

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

mod client;
pub mod elf;
mod file;
mod memory;
mod process;
mod protocol;
pub mod service;
mod stack;
mod syscall;

pub use client::{Control, Program};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidRequest,
    InvalidExecutable,
    NoMemory,
    Filesystem(fs::Error),
    Device(devmgr::Error),
    Runtime(runtime::Error),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExitStatus {
    pub code: u8,
    pub signal: u8,
}

impl ExitStatus {
    pub(crate) fn encode(self) -> usize {
        (usize::from(self.code) << 8) | usize::from(self.signal)
    }

    pub(crate) fn decode(raw: usize) -> Self {
        Self {
            code: (raw >> 8) as u8,
            signal: (raw & 127) as u8,
        }
    }
}

impl From<runtime::Error> for Error {
    fn from(error: runtime::Error) -> Self {
        Self::Runtime(error)
    }
}
impl From<fs::Error> for Error {
    fn from(error: fs::Error) -> Self {
        Self::Filesystem(error)
    }
}
impl From<devmgr::Error> for Error {
    fn from(error: devmgr::Error) -> Self {
        Self::Device(error)
    }
}
