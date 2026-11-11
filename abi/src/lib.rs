// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Register conventions and shared types for native object calls.

#![no_std]
#![forbid(unsafe_code)]

pub mod device;
pub mod dma;
mod handle;
pub mod ipc;
pub mod logger;
pub mod memory;
pub mod method;
mod object;
pub mod process;
mod status;
pub mod thread;
pub mod trap;

pub use handle::{Handle, Rights};
pub use object::ObjectType;
pub use status::{Error, Return};
