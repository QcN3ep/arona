// SPDX-License-Identifier: MPL-2.0

use crate::{ObjectType, Rights};

macro_rules! methods {
    ($name:ident { $($method:ident = $number:literal => $right:ident;)* }) => {
        pub mod $name {
            use crate::Rights;
            $(pub const $method: u64 = $number;)*
            pub(super) const fn required(method: u64) -> Option<Rights> {
                match method { $($method => Some(Rights::$right),)* _ => None }
            }
        }
    };
}
methods!(task {
    ALLOCATE = 16 => FRAME_ALLOCATE;
    MAP = 17 => MAP_MEMORY;
    UNMAP = 18 => MAP_MEMORY;
    PROTECT = 19 => MAP_MEMORY;
    RESERVE = 20 => MAP_MEMORY;
    UNRESERVE = 21 => MAP_MEMORY;
    QUERY = 22 => MAP_MEMORY;
});
methods!(memory {
    SIZE = 16 => NONE;
    READ = 17 => READ;
    WRITE = 18 => WRITE;
});

pub const INFO: u64 = 0;
pub const CLOSE: u64 = 1;
pub const DUPLICATE: u64 = 2;

pub const fn required_rights(kind: ObjectType, method: u64) -> Option<Rights> {
    match method {
        INFO | CLOSE => Some(Rights::NONE),
        DUPLICATE => Some(Rights::DUPLICATE),
        _ => match kind {
            ObjectType::Task => task::required(method),
            ObjectType::FrameAllocation => memory::required(method),
            _ => None,
        },
    }
}
