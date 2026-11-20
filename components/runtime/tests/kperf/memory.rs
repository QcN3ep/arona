// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Measure both directions of copying between the caller and an isolated Task.

use alloc::vec;
use runtime::{Rights, memory::Permissions, process};
use testkit::{kperf, perf::Perf};

fn copy<const SIZE: usize, const OFFSET: usize>(perf: &mut Perf, write: bool) {
    let task = process::task()
        .create_task(
            Rights::FRAME_ALLOCATE
                | Rights::MAP_MEMORY
                | Rights::READ_MEMORY
                | Rights::WRITE_MEMORY
                | Rights::TERMINATE,
        )
        .unwrap();
    let count = (SIZE + OFFSET).div_ceil(4096) as u64;
    let frames = task
        .allocate(count, Rights::READ_WRITE | Rights::MAP)
        .unwrap();
    let mapping = task
        .map(&frames, 0, count, None, Permissions::ReadWrite)
        .unwrap();
    let address = mapping.range().unwrap().start + OFFSET;
    let input = vec![0x5a; SIZE];
    let mut output = vec![0; SIZE];
    task.write_memory(address, &input).unwrap();
    if write {
        perf.iter(|| {
            task.write_memory(address, core::hint::black_box(&input))
                .unwrap()
        });
        task.read_memory(address, &mut output).unwrap();
    } else {
        perf.iter(|| {
            task.read_memory(address, core::hint::black_box(&mut output))
                .unwrap()
        });
    }
    assert_eq!(output, input);
    drop(mapping);
    task.terminate(0).unwrap();
}

#[kperf]
fn read_64b(perf: &mut Perf) {
    copy::<64, 0>(perf, false);
}
#[kperf]
fn read_4kib(perf: &mut Perf) {
    copy::<4096, 0>(perf, false);
}
#[kperf]
fn read_16kib(perf: &mut Perf) {
    copy::<16384, 0>(perf, false);
}
#[kperf]
fn read_64kib(perf: &mut Perf) {
    copy::<65536, 0>(perf, false);
}
#[kperf]
fn read_cross_page(perf: &mut Perf) {
    copy::<64, 4093>(perf, false);
}
#[kperf]
fn write_64b(perf: &mut Perf) {
    copy::<64, 0>(perf, true);
}
#[kperf]
fn write_4kib(perf: &mut Perf) {
    copy::<4096, 0>(perf, true);
}
#[kperf]
fn write_16kib(perf: &mut Perf) {
    copy::<16384, 0>(perf, true);
}
#[kperf]
fn write_64kib(perf: &mut Perf) {
    copy::<65536, 0>(perf, true);
}
#[kperf]
fn write_cross_page(perf: &mut Perf) {
    copy::<64, 4093>(perf, true);
}
