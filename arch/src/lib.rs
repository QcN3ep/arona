// SPDX-License-Identifier: MPL-2.0

#![no_std]

pub mod boot;

#[cfg(target_arch = "riscv64")]
pub mod riscv64;
