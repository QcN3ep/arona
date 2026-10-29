// SPDX-License-Identifier: MPL-2.0

//! 设备描述的查询与用户态驱动。

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

// TODO: 接入 PCI 功能清单与资源授权并匹配驱动。
// TODO: 实现 VirtIO MMIO 与 PCI transport 的协商、队列管理和设备复位。
// TODO: 实现 VirtIO 块设备的读写与缓存刷新。
// TODO: 实现 VirtIO 网络设备的收发与链路状态管理。
// TODO: 实现 VirtIO 随机数设备的熵源读取。
// TODO: 实现 CFI Flash 的总线宽度适配、读取、编程与擦除。

pub mod device;
pub mod uart;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Runtime(runtime::Error),
    Call(runtime::ipc::CallError),
    Unsupported,
    InvalidConfiguration,
    ClockUnavailable,
    Busy,
    Unavailable,
}

impl From<runtime::Error> for Error {
    fn from(error: runtime::Error) -> Self {
        Self::Runtime(error)
    }
}
