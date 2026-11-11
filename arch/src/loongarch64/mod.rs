// SPDX-License-Identifier: MPL-2.0

//! LoongArch 64 ISA implementation.

pub mod boot;
mod cpu;
mod execution;
mod interrupt;
pub mod paging;
mod timer;
mod trap;
