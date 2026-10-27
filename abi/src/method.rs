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
    STATE = 23 => NONE;
    CREATE = 24 => CREATE_TASK;
    CREATE_THREAD = 25 => CREATE_THREAD;
    INSTALL = 26 => INSTALL_HANDLE;
    TERMINATE = 27 => TERMINATE;
    WAIT = 28 => WAIT;
});
methods!(memory {
    SIZE = 16 => NONE;
    READ = 17 => READ;
    WRITE = 18 => WRITE;
});

methods!(thread {
    STATE = 16 => NONE;
    START = 17 => START;
    TERMINATE = 18 => TERMINATE;
    WAIT = 19 => WAIT;
    EXIT = 20 => NONE;
    YIELD = 21 => NONE;
    SLEEP = 22 => NONE;
    NOW = 23 => NONE;
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
            ObjectType::Thread => thread::required(method),
            ObjectType::FrameAllocation => memory::required(method),
            _ => None,
        },
    }
}
