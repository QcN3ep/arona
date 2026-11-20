// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#![no_std]
#![forbid(unsafe_code)]
#![feature(custom_test_frameworks)]
#![test_runner(run)]

use testkit::{kperf, perf::Perf};

#[kperf]
fn empty(perf: &mut Perf) {
    perf.iter(|| ());
}

fn run(cases: &[&testkit::Case]) {
    testkit::user::run(cases, runtime::process::bootstrap().unwrap());
}
