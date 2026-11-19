// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Register conventions and shared types for native object calls.

#![no_std]
#![forbid(unsafe_code)]

pub mod device;
pub mod dma;
mod handle;
pub mod info;
pub mod ipc;
pub mod logger;
pub mod memory;
pub mod method;
mod object;
pub mod process;
pub mod register;
mod status;
pub mod system;
pub mod thread;
pub mod trap;

pub use handle::{Handle, Rights};
pub use object::ObjectType;
pub use status::{Error, Return};
