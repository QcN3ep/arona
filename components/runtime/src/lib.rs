// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#![no_std]
#![cfg_attr(
    all(feature = "program", target_os = "none"),
    feature(lang_items, alloc_error_handler, thread_local, allow_internal_unstable)
)]
#![cfg_attr(all(feature = "program", target_os = "none"), allow(internal_features))]

extern crate alloc;

// TODO: Provide platform adaptation for Rust std and support for I/O, synchronization, time, and networking.

mod call;
pub mod device;
pub mod dma;
mod handle;
pub mod ipc;
pub mod logger;
pub mod memory;
pub mod thread;
pub mod trap;

#[cfg(all(feature = "program", target_os = "none"))]
mod heap;
pub mod process;
#[cfg(all(feature = "program", target_os = "none"))]
mod start;
#[cfg(all(feature = "program", target_os = "none"))]
pub mod sync;
#[cfg(all(feature = "program", target_os = "none"))]
pub mod tls;

pub use abi::{Error as KernelError, ObjectType, Rights};
pub use call::Error;
pub use handle::{AnyHandle, BorrowedHandle, Handle, HandleInfo, Object};

mod sealed {
    pub trait Object {}
}

macro_rules! object_types {
    ($($kind:ident = $id:literal => [$($right:ident),*];)*) => { $(
        pub enum $kind {}
        impl sealed::Object for $kind {}
        impl Object for $kind { const TYPE: ObjectType = ObjectType::$kind; }
    )* };
}
abi::objects!(object_types);
