// SPDX-License-Identifier: MPL-2.0

#![no_std]

pub mod boot;
pub mod device;
pub mod hardware;

#[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
mod allocator;

#[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
mod execution;

#[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
mod paging;

#[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
mod timer;

#[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
mod trap;

#[cfg(target_arch = "loongarch64")]
pub mod loongarch64;

#[cfg(target_arch = "riscv64")]
pub mod riscv64;
