// SPDX-License-Identifier: MPL-2.0

//! 原生 Rust 程序的内核能力与运行环境。

#![no_std]

mod call;
mod handle;

pub use abi::{ObjectType, Rights};
pub use call::Error;
pub use handle::{BorrowedHandle, Handle, HandleInfo, Object};

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
