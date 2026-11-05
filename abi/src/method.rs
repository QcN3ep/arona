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
    CHANNEL = 29 => CREATE_CHANNEL;
    EVENT = 30 => CREATE_EVENT;
    TIMER = 31 => CREATE_TIMER;
    READ_MEMORY = 32 => READ_MEMORY;
    WRITE_MEMORY = 33 => WRITE_MEMORY;
    TRAP_PORT = 34 => SET_TRAP;
    SET_TRAP = 35 => SET_TRAP;
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
    WAIT_MANY = 24 => NONE;
    SET_STATE = 25 => SET_STATE;
    INTERRUPT = 26 => INTERRUPT;
    ID = 27 => NONE;
});

methods!(trap_port { RECEIVE = 16 => READ; });
methods!(trap_continuation { STATE = 16 => READ; RESUME = 17 => SET_STATE; });

methods!(channel {
    SEND = 16 => WRITE;
    RECEIVE = 17 => READ;
    CALL = 18 => READ_WRITE;
});
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
    STATE = 16 => READ;
    MAP = 17 => DMA_MAP;
    UNMAP = 18 => DMA_MAP;
    RESUME = 19 => DEVICE_CONTROL;
    QUIESCE = 20 => DEVICE_CONTROL;
    REVOKE = 21 => REVOKE;
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
            ObjectType::Channel => channel::required(method),
            ObjectType::Reply => reply::required(method),
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
