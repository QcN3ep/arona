// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Device description queries and userspace drivers.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

// TODO: Implement transmission, reception, and link state management for VirtIO network devices.
// TODO: Implement bus width adaptation, read, program, and erase for CFI Flash.

pub mod block;
pub mod device;
pub mod power;
pub mod rng;
pub mod rtc;
pub mod uart;
mod virtio;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Runtime(runtime::Error),
    Call(runtime::ipc::CallError),
    Unsupported,
    InvalidConfiguration,
    ClockUnavailable,
    Busy,
    Unavailable,
    InvalidRequest,
    DeviceFault,
    NotFound,
    ReadOnly,
    Denied,
}

impl From<runtime::Error> for Error {
    fn from(error: runtime::Error) -> Self {
        Self::Runtime(error)
    }
}
