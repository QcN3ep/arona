// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Register functional and performance cases for kernel and user programs.

#![no_std]
#![forbid(unsafe_code)]

pub use macros::{kperf, ktest};

pub mod perf;
pub mod user;

pub struct Case {
    pub name: &'static str,
    pub kind: CaseKind,
}

pub enum CaseKind {
    Test {
        run: fn(),
        expected: &'static [&'static str],
    },
    Perf(fn(&mut perf::Perf)),
}
