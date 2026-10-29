// SPDX-License-Identifier: MPL-2.0

//! 原生对象调用的寄存器约定与共享类型。

#![no_std]
#![forbid(unsafe_code)]

pub mod device;
mod handle;
pub mod ipc;
pub mod memory;
pub mod method;
mod object;
pub mod process;
mod status;
pub mod thread;

pub use handle::{Handle, Rights};
pub use object::ObjectType;
pub use status::{Error, Return};
