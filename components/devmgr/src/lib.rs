// SPDX-License-Identifier: MPL-2.0

//! 设备描述的查询与用户态驱动。

#![no_std]
#![forbid(unsafe_code)]

pub mod device;
pub mod uart;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Runtime(runtime::Error),
    Unsupported,
    InvalidConfiguration,
    ClockUnavailable,
    Busy,
}

impl From<runtime::Error> for Error {
    fn from(error: runtime::Error) -> Self {
        Self::Runtime(error)
    }
}
