// SPDX-License-Identifier: MPL-2.0

use spin::Mutex;

use kernel::{
    ipc::{Event, EventError, EventReference, WaitItem, WaitManyError, WaitManyResult},
    task::{Handle, HandleError, Object, Rights, Task, TaskError},
    thread::{self, Thread, ThreadState},
};

static TARGET: Mutex<Option<(Task, Handle)>> = Mutex::new(None);

pub fn run() {
    super::run_thread(delivery);
    assert!(TARGET.lock().is_none());
}

fn delivery(_: usize) -> usize {
    let task = Task::new().unwrap();
    let event = Event::new().unwrap();
    let reference = event.reference();
    let handle = task
        .insert(Object::Event(event), Rights::WAIT | Rights::DUPLICATE)
        .unwrap();
    let duplicate = task.duplicate(handle, Rights::WAIT).unwrap();
    assert_eq!(reference.signal(0), Err(EventError::InvalidArgument));
    reference.signal(3).unwrap();
    reference.signal(3).unwrap();
    let invalid = [item(handle, 1), item(duplicate, 2)];
    assert_eq!(
        task.try_wait_many(&invalid),
        Err(TaskError::Wait(WaitManyError::InvalidArgument))
    );
    assert_eq!(task.try_wait_many(&[item(handle, 1)]).unwrap().ready, 1);
    assert_eq!(task.try_wait_many(&[item(handle, 3)]).unwrap().ready, 2);
    assert_eq!(
        task.try_wait_many(&[item(handle, 3)]),
        Err(TaskError::Wait(WaitManyError::WouldBlock))
    );
    let denied = task.duplicate(handle, Rights::NONE).unwrap();
    reference.signal(1).unwrap();
    assert_eq!(
        task.try_wait_many(&[item(handle, 1), item(denied, 1)]),
        Err(TaskError::Wait(WaitManyError::Handle(HandleError::Denied)))
    );
    assert_eq!(task.try_wait_many(&[item(handle, 1)]).unwrap().ready, 1);
    task.close(denied).unwrap();
    *TARGET.lock() = Some((task.clone(), handle));
    fifo_delivery(&reference);
    task.close(duplicate).unwrap();
    let waiter = Thread::new(closed_waiter, 0).unwrap();
    waiter.start().unwrap();
    thread::yield_now().unwrap();
    assert_eq!(waiter.state(), ThreadState::Blocked);
    task.close(handle).unwrap();
    assert_eq!(reference.signal(1), Err(EventError::Closed));
    assert_eq!(waiter.wait().unwrap(), 0);
    TARGET.lock().take();
    0
}

fn fifo_delivery(event: &EventReference) {
    let first = Thread::new(waiter, 2).unwrap();
    let second = Thread::new(waiter, 1).unwrap();
    let third = Thread::new(waiter, 1).unwrap();
    for thread in [&first, &second, &third] {
        thread.start().unwrap();
    }
    thread::yield_now().unwrap();
    for thread in [&first, &second, &third] {
        assert_eq!(thread.state(), ThreadState::Blocked);
    }
    event.signal(1).unwrap();
    assert_eq!(first.state(), ThreadState::Blocked);
    assert_eq!(second.state(), ThreadState::Ready);
    assert_eq!(third.state(), ThreadState::Blocked);
    event.signal(1).unwrap();
    assert_eq!(third.state(), ThreadState::Ready);
    event.signal(2).unwrap();
    assert_eq!(first.state(), ThreadState::Ready);
    for thread in [&first, &second, &third] {
        assert_eq!(thread.wait().unwrap(), 0);
    }
}

fn waiter(mask: usize) -> usize {
    let (task, handle) = TARGET.lock().as_ref().unwrap().clone();
    assert_eq!(
        task.wait_many(&[item(handle, mask as u64)]),
        Ok(WaitManyResult {
            index: 0,
            ready: mask as u64
        })
    );
    0
}

fn closed_waiter(_: usize) -> usize {
    let (task, handle) = TARGET.lock().as_ref().unwrap().clone();
    assert_eq!(
        task.wait_many(&[item(handle, 1)]),
        Err(TaskError::Wait(WaitManyError::Event(EventError::Closed)))
    );
    0
}

fn item(handle: Handle, condition_mask: u64) -> WaitItem {
    WaitItem {
        handle,
        condition_mask,
    }
}
