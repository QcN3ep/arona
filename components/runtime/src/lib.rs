// SPDX-License-Identifier: MPL-2.0

#![no_std]
#![cfg_attr(
    all(feature = "program", target_os = "none"),
    feature(lang_items, alloc_error_handler, thread_local, allow_internal_unstable)
)]
#![cfg_attr(all(feature = "program", target_os = "none"), allow(internal_features))]

#[cfg(all(feature = "program", target_os = "none"))]
extern crate alloc;

mod call;
mod handle;
pub mod ipc;
pub mod memory;
pub mod thread;

#[cfg(all(feature = "program", target_os = "none"))]
mod heap;
pub mod process;
#[cfg(all(feature = "program", target_os = "none"))]
mod start;
#[cfg(all(feature = "program", target_os = "none"))]
pub mod tls;

pub use abi::{ObjectType, Rights};
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
