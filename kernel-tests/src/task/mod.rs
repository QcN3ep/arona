// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

mod handle;
mod lifecycle;

pub fn run() {
    handle::run();
    lifecycle::run();
}
