// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use kernel::{
    hardware,
    ipc::{Event, EventError, Timer, TimerError, WaitItem, WaitManyError},
    task::{Object, Rights, Task, TaskError},
    thread,
};

pub fn run() {
    super::run_thread(deadlines);
}

fn deadlines(_: usize) -> usize {
    let task = Task::new().unwrap();
    let event = Event::new().unwrap();
    let reference = event.reference();
    let timer = Timer::new(reference.clone(), 1).unwrap();
    let second = Timer::new(reference.clone(), 2).unwrap();
    assert!(matches!(
        Timer::new(reference.clone(), 0),
        Err(TimerError::InvalidArgument)
    ));
    let handle = task.insert(Object::Event(event), Rights::WAIT).unwrap();
    let items = [WaitItem {
        handle,
        condition_mask: 3,
    }];
    assert_eq!(timer.arm(u64::MAX), Err(TimerError::InvalidArgument));
    timer.cancel().unwrap();
    timer.arm(0).unwrap();
    timer.cancel().unwrap();
    assert_eq!(task.try_wait_many(&items).unwrap().ready, 1);
    for _ in 0..4 {
        let now = hardware::timer().now_ns().unwrap();
        let replaced = now + 30_000_000;
        timer.arm(now + 2_000_000).unwrap();
        timer.arm(replaced).unwrap();
        second.arm(now + 10_000_000).unwrap();
        for mask in [2, 1] {
            let selected = [WaitItem {
                handle,
                condition_mask: mask,
            }];
            assert_eq!(
                task.wait_many_until(&selected, now + 1_000_000_000)
                    .unwrap()
                    .ready,
                mask
            );
        }
        assert!(hardware::timer().now_ns().unwrap() >= replaced);
        assert_eq!(
            task.try_wait_many(&items),
            Err(TaskError::Wait(WaitManyError::WouldBlock))
        );
    }
    let now = hardware::timer().now_ns().unwrap();
    timer.arm(now + 2_000_000).unwrap();
    timer.cancel().unwrap();
    timer.cancel().unwrap();
    assert_eq!(
        task.wait_many_until(&items, now + 10_000_000),
        Err(TaskError::Wait(WaitManyError::TimedOut))
    );
    let borrowed = second.reference();
    second
        .arm(hardware::timer().now_ns().unwrap() + 2_000_000)
        .unwrap();
    drop(second);
    assert_eq!(borrowed.arm(0), Err(TimerError::Closed));
    assert_eq!(borrowed.cancel(), Err(TimerError::Closed));
    thread::sleep_until(hardware::timer().now_ns().unwrap() + 5_000_000).unwrap();
    assert_eq!(
        task.try_wait_many(&items),
        Err(TaskError::Wait(WaitManyError::WouldBlock))
    );
    timer
        .arm(hardware::timer().now_ns().unwrap() + 2_000_000)
        .unwrap();
    task.close(handle).unwrap();
    assert_eq!(reference.signal(1), Err(EventError::Closed));
    assert_eq!(timer.arm(0), Err(TimerError::Closed));
    timer.cancel().unwrap();
    thread::sleep_until(hardware::timer().now_ns().unwrap() + 5_000_000).unwrap();
    0
}
