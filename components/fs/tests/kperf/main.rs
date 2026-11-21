// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#![no_std]
#![forbid(unsafe_code)]
#![feature(custom_test_frameworks)]
#![test_runner(run)]

extern crate alloc;

#[macro_use]
mod cases;
mod disk;

use testkit::perf::Perf;

mod tmpfs {
    use super::*;
    fn setup() -> (fs::tmpfs::Tmpfs, Option<disk::Driver>) {
        (fs::tmpfs::Tmpfs::new().unwrap(), None)
    }
    cases!(fs::tmpfs::Tmpfs);
}

mod ext4 {
    use super::*;
    fn setup() -> (fs::ext4::Ext4, Option<disk::Driver>) {
        let (disk, driver) = disk::Driver::start();
        let length = disk.capacity();
        (
            fs::ext4::Ext4::mount(disk, 0, length).unwrap(),
            Some(driver),
        )
    }
    cases!(fs::ext4::Ext4);
}

mod fat32 {
    use super::*;
    fn setup() -> (fs::fat32::Fat32, Option<disk::Driver>) {
        let (disk, driver) = disk::Driver::start();
        let length = disk.capacity();
        (
            fs::fat32::Fat32::mount(disk, 0, length).unwrap(),
            Some(driver),
        )
    }
    cases!(fs::fat32::Fat32);
}

fn run(cases: &[&testkit::Case]) {
    testkit::user::run(cases, runtime::process::bootstrap().unwrap());
}
