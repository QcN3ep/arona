// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use alloc::vec::Vec;
use spin::Mutex;

use kernel::{
    hardware,
    ipc::{
        Channel, Event, Message, PEER_CLOSED, READABLE, Timer, WRITABLE, WaitItem, WaitManyError,
        WaitManyResult,
    },
    task::{Handle, Object, ObjectReference, ObjectType, Rights, Task, TaskError},
    thread::{self, Thread, ThreadState},
};

static TARGET: Mutex<Option<(Task, [Handle; 2])>> = Mutex::new(None);

pub fn run() {
    super::run_thread(readiness);
    super::run_thread(large_selection);
    super::run_thread(channel_readiness);
    super::run_thread(channel_writable);
    super::run_thread(exit_readiness);
    assert!(TARGET.lock().is_none());
}

fn large_selection(_: usize) -> usize {
    let task = Task::new().unwrap();
    let mut items = Vec::new();
    for _ in 0..1024 {
        let handle = task
            .insert(
                Object::Event(Event::new().unwrap()),
                Rights::WAIT | Rights::DUPLICATE,
            )
            .unwrap();
        items.push(item(handle, 1));
    }
    let duplicate = task.duplicate(items[0].handle, Rights::WAIT).unwrap();
    items.push(item(duplicate, 1));
    assert_eq!(
        task.try_wait_many(&items),
        Err(TaskError::Wait(WaitManyError::InvalidArgument))
    );
    items.pop();
    let last = items.len() - 1;
    let ObjectReference::Event(event) = task
        .get(items[last].handle, ObjectType::Event, Rights::WAIT)
        .unwrap()
    else {
        unreachable!()
    };
    let timer = Timer::new(event, 1).unwrap();
    let interrupts = hardware::cpu().disable_interrupts();
    let deadline = hardware::timer().now_ns().unwrap() + 1_000_000;
    timer.arm(deadline).unwrap();
    while hardware::timer().now_ns().unwrap() < deadline {
        core::hint::spin_loop();
    }
    assert_eq!(
        task.try_wait_many(&items),
        Ok(WaitManyResult {
            index: last,
            ready: 1
        })
    );
    assert_eq!(
        task.try_wait_many(&items),
        Err(TaskError::Wait(WaitManyError::WouldBlock))
    );
    assert_eq!(
        hardware::cpu().interrupt_state(),
        hardware::cpu::InterruptState::Disabled
    );
    hardware::cpu().restore_interrupts(interrupts);
    0
}

fn readiness(_: usize) -> usize {
    let task = Task::new().unwrap();
    let first = Event::new().unwrap();
    let second = Event::new().unwrap();
    let first_ref = first.reference();
    let second_ref = second.reference();
    let handles = [
        task.insert(Object::Event(first), Rights::WAIT).unwrap(),
        task.insert(Object::Event(second), Rights::WAIT).unwrap(),
    ];
    let items = [item(handles[0], 1), item(handles[1], 1)];
    assert_eq!(
        task.try_wait_many(&[]),
        Err(TaskError::Wait(WaitManyError::InvalidArgument))
    );
    assert_eq!(
        task.try_wait_many(&[items[0]; 2]),
        Err(TaskError::Wait(WaitManyError::InvalidArgument))
    );
    assert_eq!(
        task.try_wait_many(&[item(handles[0], 0)]),
        Err(TaskError::Wait(WaitManyError::InvalidArgument))
    );
    first_ref.signal(1).unwrap();
    second_ref.signal(1).unwrap();
    assert_eq!(
        task.wait_many_until(&items, 0),
        Ok(WaitManyResult { index: 0, ready: 1 })
    );
    assert_eq!(
        task.try_wait_many(&items),
        Ok(WaitManyResult { index: 1, ready: 1 })
    );
    assert_eq!(
        task.wait_many_until(&items, 0),
        Err(TaskError::Wait(WaitManyError::TimedOut))
    );
    *TARGET.lock() = Some((task.clone(), handles));
    let waiter = Thread::new(multiple, 0).unwrap();
    waiter.start().unwrap();
    thread::yield_now().unwrap();
    assert_eq!(waiter.state(), ThreadState::Blocked);
    second_ref.signal(1).unwrap();
    first_ref.signal(1).unwrap();
    assert_eq!(waiter.wait().unwrap(), 0);
    assert_eq!(
        task.try_wait_many(&items),
        Ok(WaitManyResult { index: 0, ready: 1 })
    );
    let deadline = hardware::timer().now_ns().unwrap() + 1_000_000;
    assert_eq!(
        task.wait_many_until(&items, deadline),
        Err(TaskError::Wait(WaitManyError::TimedOut))
    );
    first_ref.signal(1).unwrap();
    assert_eq!(task.try_wait_many(&items).unwrap().index, 0);
    TARGET.lock().take();
    0
}

fn multiple(_: usize) -> usize {
    let (task, handles) = TARGET.lock().as_ref().unwrap().clone();
    assert_eq!(
        task.wait_many(&[item(handles[0], 1), item(handles[1], 1)]),
        Ok(WaitManyResult { index: 1, ready: 1 })
    );
    0
}

fn channel_readiness(_: usize) -> usize {
    let task = Task::new().unwrap();
    let (sender, receiver) = Channel::pair().unwrap();
    let handle = task
        .insert(Object::Channel(receiver), Rights::WAIT)
        .unwrap();
    let items = [item(handle, READABLE | PEER_CLOSED)];
    let peer = task
        .insert(Object::Channel(sender.clone()), Rights::WAIT)
        .unwrap();
    assert_eq!(
        task.try_wait_many(&[item(peer, READABLE), items[0]]),
        Err(TaskError::Wait(WaitManyError::WouldBlock))
    );
    task.close(peer).unwrap();
    *TARGET.lock() = Some((task.clone(), [handle; 2]));
    let waiter = Thread::new(channel_waiter, 0).unwrap();
    waiter.start().unwrap();
    thread::yield_now().unwrap();
    assert_eq!(waiter.state(), ThreadState::Blocked);
    sender.send(Message::new(42, &[]).unwrap()).unwrap();
    drop(sender);
    assert_eq!(waiter.wait().unwrap(), 0);
    assert_eq!(
        task.try_wait_many(&items).unwrap().ready,
        READABLE | PEER_CLOSED
    );
    TARGET.lock().take();
    task.close(handle).unwrap();
    0
}

fn channel_writable(_: usize) -> usize {
    let task = Task::new().unwrap();
    let (sender, receiver) = Channel::pair().unwrap();
    for _ in 0..32 {
        sender.try_send(Message::new(0, &[]).unwrap()).unwrap();
    }
    let handle = task.insert(Object::Channel(sender), Rights::WAIT).unwrap();
    *TARGET.lock() = Some((task.clone(), [handle; 2]));
    let waiter = Thread::new(channel_waiter, 1).unwrap();
    waiter.start().unwrap();
    thread::yield_now().unwrap();
    assert_eq!(waiter.state(), ThreadState::Blocked);
    receiver.try_receive().unwrap();
    assert_eq!(waiter.wait().unwrap(), 0);
    TARGET.lock().take();
    task.close(handle).unwrap();
    0
}

fn channel_waiter(writable: usize) -> usize {
    let (task, handles) = TARGET.lock().as_ref().unwrap().clone();
    let mask = if writable == 1 {
        WRITABLE
    } else {
        READABLE | PEER_CLOSED
    };
    let result = task.wait_many(&[item(handles[0], mask)]).unwrap();
    assert_eq!(
        result.ready,
        if writable == 1 { WRITABLE } else { READABLE }
    );
    0
}

fn exit_readiness(_: usize) -> usize {
    let task = Task::new().unwrap();
    let child = Task::new().unwrap();
    let worker = Thread::new(|_| 42, 0).unwrap();
    let handles = [
        task.insert(Object::Thread(worker.clone()), Rights::WAIT)
            .unwrap(),
        task.insert(Object::Task(child.clone()), Rights::WAIT)
            .unwrap(),
    ];
    *TARGET.lock() = Some((task.clone(), handles));
    let waiter = Thread::new(exit_waiter, 0).unwrap();
    waiter.start().unwrap();
    thread::yield_now().unwrap();
    assert_eq!(waiter.state(), ThreadState::Blocked);
    worker.start().unwrap();
    thread::yield_now().unwrap();
    child.terminate(7);
    assert_eq!(waiter.wait().unwrap(), 0);
    let now = hardware::timer().now_ns().unwrap();
    assert_eq!(
        task.wait_many_until(&[item(handles[1], 1)], now)
            .unwrap()
            .ready,
        1
    );
    TARGET.lock().take();
    0
}

fn exit_waiter(_: usize) -> usize {
    let (task, handles) = TARGET.lock().as_ref().unwrap().clone();
    assert_eq!(task.wait_many(&[item(handles[0], 1)]).unwrap().ready, 1);
    assert_eq!(task.wait_many(&[item(handles[1], 1)]).unwrap().ready, 1);
    0
}

fn item(handle: Handle, condition_mask: u64) -> WaitItem {
    WaitItem {
        handle,
        condition_mask,
    }
}
