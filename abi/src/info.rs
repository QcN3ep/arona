// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Object information topics and their permission contracts.

use crate::{ObjectType, Rights};

/// Returns the referenced object's type number and the handle's rights.
pub const HANDLE: u64 = 0;

macro_rules! topics {
    ($name:ident { $($(#[$doc:meta])* $topic:ident = $number:literal => $right:ident;)* }) => {
        pub mod $name {
            $(
                $(#[$doc])*
                #[doc = concat!("Required handle rights: `", stringify!($right), "`.")]
                pub const $topic: u64 = $number;
            )*

            pub(super) const fn required(topic: u64) -> Option<crate::Rights> {
                match topic {
                    $($topic => Some(crate::Rights::$right),)*
                    _ => None,
                }
            }
        }
    };
}

topics!(task {
    /// Returns `[Lifecycle, exit_status]`; live tasks report zero status.
    /// Exited tasks have completed cleanup and retain their final status.
    LIFECYCLE = 16 => WAIT;
    /// Reads AddressQuery and writes one AddressRegion snapshot.
    /// No matching region returns zero bytes without touching output.
    /// BufferTooSmall reports AddressRegion::SIZE without changing output.
    ADDRESS_SPACE = 17 => MAP_MEMORY;
});
topics!(thread {
    /// Returns `[Lifecycle, exit_status]` without exposing scheduler states.
    LIFECYCLE = 16 => WAIT;
    /// Returns `[thread_id, 0]`, unique for the lifetime of the kernel.
    IDENTITY = 17 => READ;
    /// Returns `[cpu_nanoseconds, 0]`, including the current execution interval.
    CPU_TIME = 18 => READ;
});
topics!(memory {
    /// Returns `[frame_count, 0]` for this allocation.
    SIZE = 16 => NONE;
    /// Returns `[mapping_references, owned_capabilities]`.
    /// In-flight capabilities count as owners; temporary kernel borrows do not.
    /// Counts may change concurrently after the query.
    USAGE = 17 => READ;
});
topics!(kernel_log {
    /// Returns `[capacity_bytes, 0]` for the referenced log buffer.
    CAPACITY = 16 => READ;
});
topics!(dma {
    /// Returns `[DmaState, 0]` for the referenced domain.
    STATE = 16 => READ;
});
topics!(clock {
    /// Returns `[monotonic_nanoseconds, 0]` using the common deadline timebase.
    MONOTONIC = 16 => READ;
});
topics!(system {
    /// Returns `[total_physical_bytes, free_physical_bytes]`.
    MEMORY = 16 => READ;
    /// Writes a RunnableIntegral snapshot without query parameters.
    RUNNABLE_INTEGRAL = 17 => READ;
});

pub const fn required_rights(kind: ObjectType, topic: u64) -> Option<Rights> {
    if topic == HANDLE {
        return Some(Rights::NONE);
    }
    match kind {
        ObjectType::Task => task::required(topic),
        ObjectType::Thread => thread::required(topic),
        ObjectType::FrameAllocation => memory::required(topic),
        ObjectType::KernelLog => kernel_log::required(topic),
        ObjectType::DmaDomain => dma::required(topic),
        ObjectType::Clock => clock::required(topic),
        ObjectType::SystemInfo => system::required(topic),
        _ => None,
    }
}
