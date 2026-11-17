// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Measure message round trips, capability transfer, calls, and multi-object waits.

use alloc::vec::Vec;
use core::hint::black_box;
use kernel::{
    ipc::{self, Channel, Message, READABLE, WaitItem},
    task::{self, Handle, Object, ObjectType, Rights, Task, TransferRequest},
    thread::{self, ThreadState},
};

use crate::perf::Perf;

mod worker;

const WORDS: [u64; 8] = [3, 5, 8, 13, 21, 34, 55, 89];

struct Peers {
    client: Task,
    server: Task,
    sender: Handle,
    receiver: Handle,
}

impl Peers {
    fn new() -> Self {
        let client = Task::new().unwrap();
        let server = Task::new().unwrap();
        let (sender, receiver) = Channel::pair().unwrap();
        let rights = ObjectType::Channel.supported_rights();
        Self {
            sender: client.insert(Object::Channel(sender), rights).unwrap(),
            receiver: server.insert(Object::Channel(receiver), rights).unwrap(),
            client,
            server,
        }
    }
}

impl Drop for Peers {
    fn drop(&mut self) {
        self.client.terminate(0);
        self.server.terminate(0);
    }
}

/// Check object reclamation after the measured thread and its fixtures have exited.
pub fn run(perf: &mut Perf, case: fn(&mut Perf)) {
    let before = resources();
    perf.in_thread(case);
    assert_eq!(resources(), before);
}

fn resources() -> [usize; 8] {
    [
        ipc::active_channels(),
        ipc::queued_messages(),
        ipc::active_events(),
        ipc::active_replies(),
        ipc::active_selections(),
        task::registered_tasks(),
        task::allocated_task_records() as usize,
        thread::allocated_threads(),
    ]
}

/// Send and receive a 64-byte message in both directions without Task handles.
pub fn channel_roundtrip(perf: &mut Perf) {
    let (client, server) = Channel::pair().unwrap();
    let message = Message::new(1, &WORDS).unwrap();
    let mut response = message;
    perf.iter(|| {
        client.try_send(black_box(message)).unwrap();
        server.try_send(server.try_receive().unwrap()).unwrap();
        response = client.try_receive().unwrap();
        response
    });
    assert_eq!(response, message);
    assert_eq!(ipc::queued_messages(), 0);
}

/// Send and receive a 64-byte message in both directions through Task handles.
pub fn task_roundtrip(perf: &mut Perf) {
    let peers = Peers::new();
    let message = Message::new(1, &WORDS).unwrap();
    let mut response = message;
    perf.iter(|| {
        peers
            .client
            .try_send(peers.sender, black_box(message), &[])
            .unwrap();
        let request = peers.server.try_receive(peers.receiver, 0).unwrap();
        peers
            .server
            .try_send(peers.receiver, request.message, &[])
            .unwrap();
        response = peers.client.try_receive(peers.sender, 0).unwrap().message;
        response
    });
    assert_eq!(response, message);
    assert_eq!(ipc::queued_messages(), 0);
}

/// Move the same N event handles to the server and back, including both message deliveries.
pub fn transfer<const N: usize>(perf: &mut Perf) {
    let peers = Peers::new();
    let rights = ObjectType::Event.supported_rights();
    let mut transfers: [_; N] = core::array::from_fn(|_| TransferRequest {
        handle: peers
            .client
            .insert(Object::Event(ipc::Event::new().unwrap()), rights)
            .unwrap(),
        rights,
    });
    let message = Message::new(1, &WORDS).unwrap();
    let mut response = message;
    perf.iter(|| {
        peers
            .client
            .try_send(peers.sender, black_box(message), &transfers)
            .unwrap();
        let received = peers.server.try_receive(peers.receiver, N).unwrap();
        let returning: [_; N] = core::array::from_fn(|index| TransferRequest {
            handle: received.handles()[index],
            rights,
        });
        peers
            .server
            .try_send(peers.receiver, received.message, &returning)
            .unwrap();
        let received = peers.client.try_receive(peers.sender, N).unwrap();
        transfers = core::array::from_fn(|index| TransferRequest {
            handle: received.handles()[index],
            rights,
        });
        response = received.message;
        response
    });
    assert_eq!(response, message);
    for transfer in transfers {
        peers
            .client
            .get(transfer.handle, ObjectType::Event, rights)
            .unwrap();
    }
    assert_eq!(ipc::queued_messages(), 0);
}

/// Complete a call without scheduling, including allocation and release of the call and reply.
pub fn call_ready(perf: &mut Perf) {
    let peers = Peers::new();
    let message = Message::new(1, &WORDS).unwrap();
    let mut response = message;
    perf.iter(|| {
        let call = peers
            .client
            .start_call(peers.sender, black_box(message), &[])
            .unwrap();
        let request = peers.server.try_receive(peers.receiver, 1).unwrap();
        let reply = request.handles()[0];
        peers.server.reply(reply, request.message, &[]).unwrap();
        peers.server.close(reply).unwrap();
        response = call.try_receive().unwrap().message;
        response
    });
    assert_eq!(response, message);
    assert_eq!(ipc::active_replies(), 0);
}

/// Complete a synchronous call while the server blocks on receive between replies.
pub fn call_blocked(perf: &mut Perf) {
    let peers = Peers::new();
    let worker = worker::start(worker::Job::Call(peers.server.clone(), peers.receiver));
    let message = Message::new(1, &WORDS).unwrap();
    let mut response = message;
    let mut rounds = 0;
    perf.iter(|| {
        response = peers
            .client
            .call(peers.sender, black_box(message), &[])
            .unwrap()
            .message;
        rounds += 1;
        response
    });
    assert_eq!(response, message);
    assert_eq!(worker.state(), ThreadState::Blocked);
    peers
        .client
        .try_send(peers.sender, Message::new(0, &[]).unwrap(), &[])
        .unwrap();
    assert_eq!(worker.wait().unwrap(), rounds);
    assert_eq!(ipc::active_replies(), 0);
}

/// Yield to another ready thread and back, including its loop counter and stop check.
pub fn yield_roundtrip(perf: &mut Perf) {
    let worker = worker::start(worker::Job::Yield);
    let mut rounds = 0;
    perf.iter(|| {
        thread::yield_now().unwrap();
        rounds += 1;
    });
    worker::stop_yield();
    // The initial handshake also yields once, outside the measurement interval.
    assert_eq!(worker.wait().unwrap(), rounds + 1);
}

/// Resolve N handles and find the last readable channel without consuming its message.
pub fn wait_ready<const N: usize>(perf: &mut Perf) {
    let task = Task::new().unwrap();
    let mut senders = Vec::with_capacity(N);
    let items: Vec<_> = (0..N)
        .map(|_| {
            let (sender, receiver) = Channel::pair().unwrap();
            senders.push(sender);
            WaitItem {
                handle: task
                    .insert(Object::Channel(receiver), Rights::READ | Rights::WAIT)
                    .unwrap(),
                condition_mask: READABLE,
            }
        })
        .collect();
    senders[N - 1]
        .try_send(Message::new(1, &WORDS).unwrap())
        .unwrap();
    let mut result = None;
    perf.iter(|| {
        result = Some(task.try_wait_many(black_box(&items)).unwrap());
        result
    });
    assert_eq!(
        result.unwrap(),
        ipc::WaitManyResult {
            index: N - 1,
            ready: READABLE
        }
    );
    task.terminate(0);
}

/// Send a control message, block on N events, and resume after the worker signals the last.
pub fn wait_blocked<const N: usize>(perf: &mut Perf) {
    let task = Task::new().unwrap();
    let mut last = None;
    let items: Vec<_> = (0..N)
        .map(|_| {
            let event = ipc::Event::new().unwrap();
            last = Some(event.reference());
            WaitItem {
                handle: task.insert(Object::Event(event), Rights::WAIT).unwrap(),
                condition_mask: 1,
            }
        })
        .collect();
    let (control, receiver) = Channel::pair().unwrap();
    let worker = worker::start(worker::Job::Signal(receiver, last.unwrap()));
    let mut result = None;
    let mut rounds = 0;
    perf.iter(|| {
        control.try_send(Message::new(1, &WORDS).unwrap()).unwrap();
        result = Some(task.wait_many(black_box(&items)).unwrap());
        rounds += 1;
        result
    });
    assert_eq!(
        result.unwrap(),
        ipc::WaitManyResult {
            index: N - 1,
            ready: 1
        }
    );
    assert_eq!(worker.state(), ThreadState::Blocked);
    control.try_send(Message::new(0, &[]).unwrap()).unwrap();
    assert_eq!(worker.wait().unwrap(), rounds);
    assert_eq!(ipc::active_selections(), 0);
    task.terminate(0);
}
