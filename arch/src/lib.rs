// SPDX-License-Identifier: MPL-2.0

#![no_std]

pub mod boot;
pub mod device;

#[cfg(target_arch = "loongarch64")]
pub mod loongarch64;

#[cfg(target_arch = "riscv64")]
pub mod riscv64;
