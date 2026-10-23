// SPDX-License-Identifier: MPL-2.0

mod handle;
mod lifecycle;

pub fn run() {
    handle::run();
    lifecycle::run();
}
