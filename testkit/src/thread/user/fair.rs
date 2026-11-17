// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! CPU share and runnable-thread lifecycle scenarios for CFS.

use super::{BUSY, Fixture, wait_dead};
use kernel::{
    hardware,
    thread::{self, ThreadError},
};

pub(super) fn run() {
    fair_shares(false);
    fair_shares(true);
    sleeping_contender();
    queued_cancellation();
}

fn sleeping_contender() {
    let fixture = Fixture::new(42, BUSY);
    fixture.thread.start().unwrap();
    for _ in 0..4 {
        let before = fixture.thread.cpu_time_ns().unwrap();
        thread::sleep_until(hardware::timer().now_ns().unwrap() + 20_000_000).unwrap();
        assert_eq!(fixture.entered(), 42);
        assert_eq!(fixture.marker(), 0);
        assert!(fixture.thread.cpu_time_ns().unwrap() > before);
    }
    fixture.thread.terminate(19).unwrap();
    wait_dead(&fixture.thread);
    fixture.assert_exit(19);
}

fn fair_shares(yielding: bool) {
    let fixtures: [Fixture; 3] = core::array::from_fn(|index| {
        let rounds = if yielding && index == 1 {
            u64::MAX
        } else {
            BUSY
        };
        Fixture::new(42 + index, rounds)
    });
    for fixture in &fixtures {
        assert_eq!(fixture.thread.nice(), 0);
        assert_eq!(
            fixture.thread.set_nice(-21),
            Err(ThreadError::InvalidPriority)
        );
        assert_eq!(
            fixture.thread.set_nice(20),
            Err(ThreadError::InvalidPriority)
        );
        assert_eq!(fixture.thread.nice(), 0);
        fixture.thread.start().unwrap();
    }
    assert_shares(&fixtures, [1024; 3], yielding);
    for (fixture, nice) in fixtures.iter().zip([-5, 0, 5]) {
        fixture.thread.set_nice(nice).unwrap();
        assert_eq!(fixture.thread.nice(), nice);
    }
    assert_shares(&fixtures, [3121, 1024, 335], yielding);
    for (fixture, nice) in fixtures.iter().zip([5, 0, -5]) {
        fixture.thread.set_nice(nice).unwrap();
    }
    assert_shares(&fixtures, [335, 1024, 3121], yielding);
    for fixture in fixtures {
        fixture.thread.terminate(19).unwrap();
        wait_dead(&fixture.thread);
        fixture.assert_exit(19);
        assert_eq!(fixture.thread.set_nice(0), Err(ThreadError::InvalidState));
    }
}

fn assert_shares(fixtures: &[Fixture; 3], mut weights: [u64; 3], yielding: bool) {
    let before: [u64; 3] =
        core::array::from_fn(|index| fixtures[index].thread.cpu_time_ns().unwrap());
    thread::sleep_until(hardware::timer().now_ns().unwrap() + 120_000_000).unwrap();
    let elapsed: [u64; 3] =
        core::array::from_fn(|index| fixtures[index].thread.cpu_time_ns().unwrap() - before[index]);
    let mut total: u64 = elapsed.iter().sum();
    assert!(
        total >= 60_000_000,
        "CPU-bound threads failed to progress: {elapsed:?}"
    );
    if yielding {
        let entitled = total * weights[1] / weights.iter().sum::<u64>();
        assert!(elapsed[1] > 0 && elapsed[1] < entitled + 6_000_000);
        total -= elapsed[1];
        weights[1] = 0;
    }
    let total_weight: u64 = weights.iter().sum();
    for index in 0..3 {
        if weights[index] == 0 {
            continue;
        }
        let expected = total * weights[index] / total_weight;
        assert!(
            elapsed[index].abs_diff(expected) < expected / 4 + 6_000_000,
            "CPU shares {elapsed:?} do not follow weights {weights:?}"
        );
    }
}

fn queued_cancellation() {
    let fixtures: [Fixture; 32] = core::array::from_fn(|index| {
        let fixture = Fixture::new(42 + index, 2);
        fixture.thread.set_nice((index % 40) as i8 - 20).unwrap();
        fixture.thread.start().unwrap();
        fixture
    });
    for offset in 0..32 {
        let index = offset * 13 % 32;
        if index % 2 == 0 {
            fixtures[index].thread.terminate(19).unwrap();
            fixtures[index].assert_exit(19);
        }
    }
    for (index, fixture) in fixtures.iter().enumerate() {
        if index % 2 != 0 {
            wait_dead(&fixture.thread);
            fixture.assert_exit(42 + index);
            assert_eq!(fixture.marker(), (42 + index) as u64);
        }
    }
}
