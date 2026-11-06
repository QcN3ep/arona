// SPDX-License-Identifier: MPL-2.0

//! 设备描述的查询与用户态驱动。

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

// TODO: 实现 VirtIO 网络设备的收发与链路状态管理。
// TODO: 实现 CFI Flash 的总线宽度适配、读取、编程与擦除。

pub mod block;
pub mod device;
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
}

impl From<runtime::Error> for Error {
    fn from(error: runtime::Error) -> Self {
        Self::Runtime(error)
    }
}
