// SPDX-License-Identifier: MPL-2.0

//! 特权内核库。

#![no_std]

pub mod boot;
pub mod logger;

pub use boot::*;
pub use log;
