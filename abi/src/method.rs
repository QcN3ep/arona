// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use crate::{ObjectType, Rights};

macro_rules! methods {
    ($name:ident { $($(#[$meta:meta])* $method:ident = $number:literal => $right:ident;)* }) => {
        pub mod $name {
            use crate::Rights;
            $(
                $(#[$meta])*
                #[doc = concat!("Required handle rights: `", stringify!($right), "`.")]
                pub const $method: u64 = $number;
            )*
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
    CREATE = 24 => CREATE_TASK;
    CREATE_THREAD = 25 => CREATE_THREAD;
    INSTALL = 26 => INSTALL_HANDLE;
    TERMINATE = 27 => TERMINATE;
    CHANNEL = 29 => CREATE_CHANNEL;
    EVENT = 30 => CREATE_EVENT;
    TIMER = 31 => CREATE_TIMER;
    READ_MEMORY = 32 => READ_MEMORY;
    WRITE_MEMORY = 33 => WRITE_MEMORY;
    TRAP_PORT = 34 => SET_TRAP;
    SET_TRAP = 35 => SET_TRAP;
    ADDRESS_RANGE = 36 => MAP_MEMORY;
    QUERY_NEXT = 37 => MAP_MEMORY;
    MEMORY_STATS = 38 => NONE;
    LOAD_AVERAGE = 39 => NONE;
    ATOMIC_COMPARE_EXCHANGE = 40 => WRITE_MEMORY;
    MAP_HUGE = 41 => MAP_MEMORY;
    RANDOM = 42 => NONE;
});
methods!(memory {
    READ = 17 => READ;
    WRITE = 18 => WRITE;
});

methods!(thread {
    START = 17 => START;
    TERMINATE = 18 => TERMINATE;
    YIELD = 21 => NONE;
    NOW = 23 => NONE;
    /// Waits on distinct sources using `[items, count, deadline, 0, 0]`.
    /// Each target requires WAIT. Events consume selected signal bits; other
    /// sources remain ready until their contents or lifecycle change.
    /// TrapPort is readable while its queue is nonempty. Empty waits require
    /// a zero items address: polling returns WouldBlock, finite deadlines
    /// return TimedOut, and an infinite wait lasts until termination.
    /// Count must not exceed the caller's handle count plus CURRENT_THREAD.
    WAIT_MANY = 24 => NONE;
    SET_STATE = 25 => SET_STATE;
    INTERRUPT = 26 => INTERRUPT;
    GET_PRIORITY = 29 => NONE;
    SET_PRIORITY = 30 => SET_STATE;
    GET_SCHEDULING = 31 => NONE;
    SET_SCHEDULING = 32 => SET_STATE;
});

methods!(trap_port { RECEIVE = 16 => READ; });
methods!(trap_continuation { STATE = 16 => READ; RESUME = 17 => SET_STATE; });

methods!(channel {
    SEND = 16 => WRITE;
    RECEIVE = 17 => READ;
    CALL = 18 => READ_WRITE;
    START_CALL = 19 => READ_WRITE;
});
methods!(call { RECEIVE = 16 => READ; CANCEL = 17 => WRITE; });
methods!(reply { SEND = 16 => WRITE; });
methods!(event { SIGNAL = 16 => SIGNAL; });
methods!(timer { ARM = 16 => WRITE; CANCEL = 17 => WRITE; });
methods!(kernel_log { READ = 16 => READ; });
methods!(device_set {
    NEXT = 16 => READ;
    DESCRIBE = 17 => READ;
    READ = 18 => READ;
    CLAIM_GROUP = 19 => RESOURCE_SPLIT;
});
methods!(device {
    MAP = 16 => MAP;
    UNMAP = 17 => MAP;
    BIND = 18 => DEVICE_CONTROL;
    UNBIND = 19 => DEVICE_CONTROL;
    ACK = 20 => IRQ_ACK;
    REVOKE = 21 => REVOKE;
    CREATE_DMA = 22 => DEVICE_CONTROL;
});
methods!(dma {
    MAP = 17 => DMA_MAP;
    UNMAP = 18 => DMA_MAP;
    RESUME = 19 => DEVICE_CONTROL;
    QUIESCE = 20 => DEVICE_CONTROL;
    REVOKE = 21 => REVOKE;
});

/// Queries `[topic, query_address, query_length, output_address, output_capacity]`.
///
/// Each topic declares its rights and fixed query and output forms. Topics with
/// register results require zero output address and capacity. Buffer results
/// return `[bytes_written, 0]`; insufficient capacity returns `BufferTooSmall`
/// with `[required_bytes, 0]` without changing the output or the object.
/// Unknown topics return `Unsupported`; malformed queries return
/// `InvalidArgument`, and missing topic rights return `Denied`.
/// Queries do not block or change the object's lifecycle.
pub const INFO: u64 = 0;
pub const CLOSE: u64 = 1;
pub const DUPLICATE: u64 = 2;

pub const fn required_rights(kind: ObjectType, method: u64, topic: u64) -> Option<Rights> {
    match method {
        INFO => crate::info::required_rights(kind, topic),
        CLOSE => Some(Rights::NONE),
        DUPLICATE => Some(Rights::DUPLICATE),
        _ => match kind {
            ObjectType::Task => task::required(method),
            ObjectType::Thread => thread::required(method),
            ObjectType::FrameAllocation => memory::required(method),
            ObjectType::Channel => channel::required(method),
            ObjectType::Reply => reply::required(method),
            ObjectType::Call => call::required(method),
            ObjectType::Event => event::required(method),
            ObjectType::Timer => timer::required(method),
            ObjectType::DeviceSet => device_set::required(method),
            ObjectType::Device => device::required(method),
            ObjectType::KernelLog => kernel_log::required(method),
            ObjectType::DmaDomain => dma::required(method),
            ObjectType::TrapPort => trap_port::required(method),
            ObjectType::TrapContinuation => trap_continuation::required(method),
        },
    }
}
