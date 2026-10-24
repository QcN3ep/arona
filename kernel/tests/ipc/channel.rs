// SPDX-License-Identifier: MPL-2.0

use arrayvec::ArrayVec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

use kernel::{
    hardware,
    ipc::{Channel, ChannelError, ChannelReference, Message},
    memory::{self, FrameOwner, FramePurpose},
    task::{
        Handle, HandleError, HandleTable, Object, ObjectReference, ObjectType, Rights, Task,
        TaskError, TransferRequest,
    },
    thread::{self, Thread, ThreadError, ThreadState, scheduler},
};

static ENDPOINTS: Mutex<[Option<Channel>; 3]> = Mutex::new([const { None }; 3]);
static REFERENCES: Mutex<[Option<ChannelReference>; 2]> = Mutex::new([const { None }; 2]);
static RECEIVED: AtomicUsize = AtomicUsize::new(0);
static DEADLINE: AtomicU64 = AtomicU64::new(0);
static TRANSFER_SENDER: Mutex<Option<TransferSender>> = Mutex::new(None);
static TRANSFER_RECEIVER: Mutex<Option<(Task, Handle, usize)>> = Mutex::new(None);

struct TransferSender {
    task: Task,
    sender: Handle,
    handle: Handle,
    expected: Result<(), TaskError>,
}

pub fn run() {
    let manager = memory::manager();
    manager.reclaim_heap();
    let before = manager.heap_stats();
    let frames = manager.stats().kernel.free_frames;
    round_trips();
    receivers_close();
    senders_close();
    receive_deadline();
    send_deadline();
    borrowed_waiters_close();
    handle_transfers();
    assert!(ENDPOINTS.lock().iter().all(Option::is_none));
    assert!(REFERENCES.lock().iter().all(Option::is_none));
    assert!(TRANSFER_SENDER.lock().is_none());
    assert!(TRANSFER_RECEIVER.lock().is_none());
    manager.reclaim_heap();
    assert_eq!(manager.heap_stats(), before);
    assert_eq!(manager.stats().kernel.free_frames, frames);
}

fn round_trips() {
    let (first, second) = Channel::pair().unwrap();
    ENDPOINTS.lock()[0] = Some(first);
    ENDPOINTS.lock()[1] = Some(second);
    let sender = Thread::new(
        |_| {
            let channel = ENDPOINTS.lock()[0].take().unwrap();
            assert_eq!(Message::new(0, &[0; 9]), Err(ChannelError::InvalidMessage));
            for round in 0..8 {
                for offset in 0..33 {
                    channel.send(message(round * 33 + offset)).unwrap();
                }
                for offset in 0..33 {
                    assert_eq!(channel.receive(), Ok(message(round * 33 + offset)));
                }
            }
            assert_eq!(channel.try_receive(), Err(ChannelError::EndOfStream));
            0
        },
        0,
    )
    .unwrap();
    let receiver = Thread::new(
        |_| {
            let channel = ENDPOINTS.lock()[1].take().unwrap();
            for round in 0..8 {
                for offset in 0..33 {
                    let expected = message(round * 33 + offset);
                    let received = channel.receive().unwrap();
                    assert_eq!(received, expected);
                    assert_eq!(received.words(), expected.words());
                }
                for offset in 0..33 {
                    channel.send(message(round * 33 + offset)).unwrap();
                }
            }
            1
        },
        0,
    )
    .unwrap();
    sender.start().unwrap();
    receiver.start().unwrap();
    scheduler::run_ready().unwrap();
    assert_eq!(sender.state(), ThreadState::Dead(0));
    assert_eq!(receiver.state(), ThreadState::Dead(1));
}

fn receivers_close() {
    RECEIVED.store(0, Ordering::Relaxed);
    let (sender, receiver) = Channel::pair().unwrap();
    let duplicate = sender.clone();
    let workers: [Thread; 3] = core::array::from_fn(|index| {
        ENDPOINTS.lock()[index] = Some(receiver.clone());
        Thread::new(
            |index| {
                let channel = ENDPOINTS.lock()[index].take().unwrap();
                loop {
                    match channel.receive() {
                        Ok(received) => {
                            let expected = RECEIVED.fetch_add(1, Ordering::Relaxed) as u64;
                            assert_eq!(received, message(expected));
                        }
                        Err(ChannelError::EndOfStream) => break,
                        result => panic!("unexpected channel receive result: {result:?}"),
                    }
                }
                assert_eq!(channel.try_send(message(0)), Err(ChannelError::PeerClosed));
                index
            },
            index,
        )
        .unwrap()
    });
    drop(receiver);
    for worker in &workers {
        worker.start().unwrap();
    }
    scheduler::run_ready().unwrap();
    assert!(
        workers
            .iter()
            .all(|worker| worker.state() == ThreadState::Blocked)
    );
    drop(sender);
    for tag in 0..3 {
        duplicate.try_send(message(tag)).unwrap();
    }
    scheduler::run_ready().unwrap();
    assert_eq!(RECEIVED.load(Ordering::Relaxed), 3);
    assert!(
        workers
            .iter()
            .all(|worker| worker.state() == ThreadState::Blocked)
    );
    for tag in 3..6 {
        duplicate.try_send(message(tag)).unwrap();
    }
    drop(duplicate);
    scheduler::run_ready().unwrap();
    assert_eq!(RECEIVED.load(Ordering::Relaxed), 6);
    for (index, worker) in workers.iter().enumerate() {
        assert_eq!(worker.state(), ThreadState::Dead(index));
    }
}

fn senders_close() {
    let (sender, receiver) = Channel::pair().unwrap();
    for tag in 0..32 {
        sender.try_send(message(tag)).unwrap();
    }
    assert_eq!(sender.try_send(message(32)), Err(ChannelError::WouldBlock));
    assert_eq!(
        sender.send(message(32)),
        Err(ChannelError::Thread(ThreadError::InvalidState))
    );
    let workers: [Thread; 3] = core::array::from_fn(|index| {
        ENDPOINTS.lock()[index] = Some(sender.clone());
        Thread::new(
            |index| {
                let channel = ENDPOINTS.lock()[index].take().unwrap();
                assert_eq!(
                    channel.send(message(32 + index as u64)),
                    Err(ChannelError::PeerClosed)
                );
                index
            },
            index,
        )
        .unwrap()
    });
    drop(sender);
    for worker in &workers {
        worker.start().unwrap();
    }
    scheduler::run_ready().unwrap();
    assert!(
        workers
            .iter()
            .all(|worker| worker.state() == ThreadState::Blocked)
    );
    assert_eq!(receiver.try_receive(), Ok(message(0)));
    assert_eq!(workers[0].state(), ThreadState::Ready);
    drop(receiver);
    scheduler::run_ready().unwrap();
    for (index, worker) in workers.iter().enumerate() {
        assert_eq!(worker.state(), ThreadState::Dead(index));
    }
}

fn receive_deadline() {
    for offset in [0, 10_000_000] {
        let (receiver, sender) = Channel::pair().unwrap();
        let workers: [Thread; 3] = core::array::from_fn(|index| {
            ENDPOINTS.lock()[index] = Some(receiver.clone());
            Thread::new(
                |index| {
                    let channel = ENDPOINTS.lock()[index].take().unwrap();
                    let deadline = if index == 0 {
                        DEADLINE.load(Ordering::Relaxed)
                    } else {
                        u64::MAX
                    };
                    let expected = match index {
                        0 => Err(ChannelError::TimedOut),
                        1 => Ok(message(73)),
                        _ => Err(ChannelError::EndOfStream),
                    };
                    assert_eq!(channel.receive_until(deadline), expected);
                    if index == 0 {
                        assert!(hardware::timer().now_ns().unwrap() >= deadline);
                    }
                    index
                },
                index,
            )
            .unwrap()
        });
        DEADLINE.store(
            hardware::timer().now_ns().unwrap() + offset,
            Ordering::Relaxed,
        );
        for worker in &workers {
            worker.start().unwrap();
        }
        scheduler::run_ready().unwrap();
        // 有限 deadline 的接收者可能已经超时完成。
        assert!(
            workers[1..]
                .iter()
                .all(|worker| worker.state() == ThreadState::Blocked)
        );
        wait_for_deadline(&workers[0]);
        sender.try_send(message(73)).unwrap();
        assert_eq!(workers[1].state(), ThreadState::Ready);
        assert_eq!(workers[2].state(), ThreadState::Blocked);
        scheduler::run_ready().unwrap();
        assert_eq!(workers[0].state(), ThreadState::Dead(0));
        assert_eq!(workers[1].state(), ThreadState::Dead(1));
        drop(sender);
        scheduler::run_ready().unwrap();
        assert_eq!(workers[2].state(), ThreadState::Dead(2));
    }
}

fn send_deadline() {
    let (sender, receiver) = Channel::pair().unwrap();
    for tag in 0..32 {
        sender.try_send(message(tag)).unwrap();
    }
    ENDPOINTS.lock()[0] = Some(sender.clone());
    let worker = Thread::new(
        |_| {
            let channel = ENDPOINTS.lock()[0].take().unwrap();
            assert_eq!(
                channel.send_until(message(32), 0),
                Err(ChannelError::TimedOut)
            );
            let deadline = hardware::timer().now_ns().unwrap() + 10_000_000;
            assert_eq!(
                channel.send_until(message(32), deadline),
                Err(ChannelError::TimedOut)
            );
            assert!(hardware::timer().now_ns().unwrap() >= deadline);
            assert_eq!(channel.try_send(message(32)), Err(ChannelError::WouldBlock));
            0
        },
        0,
    )
    .unwrap();
    worker.start().unwrap();
    scheduler::run_ready().unwrap();
    wait_for_deadline(&worker);
    scheduler::run_ready().unwrap();
    assert_eq!(worker.state(), ThreadState::Dead(0));
    for tag in 0..32 {
        assert_eq!(receiver.try_receive(), Ok(message(tag)));
    }
    assert_eq!(receiver.try_receive(), Err(ChannelError::WouldBlock));
    receiver.try_send(message(73)).unwrap();
    assert_eq!(sender.try_receive(), Ok(message(73)));
    sender.try_send(message(42)).unwrap();
    assert_eq!(receiver.try_receive(), Ok(message(42)));
}

fn borrowed_waiters_close() {
    let table = HandleTable::new();
    let (channel, peer) = Channel::pair().unwrap();
    let handle = table
        .insert(Object::Channel(channel), Rights::READ | Rights::WRITE)
        .unwrap();
    let ObjectReference::Channel(reference) = table
        .get(handle, ObjectType::Channel, Rights::READ | Rights::WRITE)
        .unwrap()
    else {
        panic!("expected channel");
    };
    for tag in 0..32 {
        reference.try_send(message(tag)).unwrap();
    }
    let workers: [Thread; 2] = core::array::from_fn(|index| {
        REFERENCES.lock()[index] = Some(reference.clone());
        Thread::new(
            |index| {
                let channel = REFERENCES.lock()[index].take().unwrap();
                if index == 0 {
                    assert_eq!(channel.receive(), Err(ChannelError::Closed));
                } else {
                    assert_eq!(channel.send(message(32)), Err(ChannelError::Closed));
                }
                index
            },
            index,
        )
        .unwrap()
    });
    for worker in &workers {
        worker.start().unwrap();
    }
    scheduler::run_ready().unwrap();
    assert!(
        workers
            .iter()
            .all(|worker| worker.state() == ThreadState::Blocked)
    );
    table.close(handle).unwrap();
    assert!(
        workers
            .iter()
            .all(|worker| worker.state() == ThreadState::Ready)
    );
    assert_eq!(peer.try_send(message(73)), Err(ChannelError::PeerClosed));
    scheduler::run_ready().unwrap();
    for (index, worker) in workers.iter().enumerate() {
        assert_eq!(worker.state(), ThreadState::Dead(index));
    }
    for tag in 0..32 {
        assert_eq!(peer.try_receive(), Ok(message(tag)));
    }
    assert_eq!(peer.try_receive(), Err(ChannelError::EndOfStream));
    assert_eq!(reference.try_receive(), Err(ChannelError::Closed));
    assert_eq!(reference.try_send(message(73)), Err(ChannelError::Closed));
}

fn handle_transfers() {
    let worker = Thread::new(
        |_| {
            with_task_channels(transfer_messages);
            transfer_endpoints();
            with_task_channels(transfer_waits);
            transfer_cycles();
            transfer_cleanup();
            with_task_channels(receive_allocation_failure);
            73
        },
        0,
    )
    .unwrap();
    worker.start().unwrap();
    scheduler::run_ready().unwrap();
    assert_eq!(worker.state(), ThreadState::Dead(73));
}

fn task_channels() -> (Task, Task, Handle, Handle) {
    let source = Task::new().unwrap();
    let target = Task::new().unwrap();
    let (sender, receiver) = Channel::pair().unwrap();
    let rights = Rights::DUPLICATE | Rights::TRANSFER | Rights::READ | Rights::WRITE;
    let sender = source.insert(Object::Channel(sender), rights).unwrap();
    let receiver = target.insert(Object::Channel(receiver), rights).unwrap();
    (source, target, sender, receiver)
}

fn with_task_channels(run: fn(Task, Task, Handle, Handle)) {
    let (source, target, sender, receiver) = task_channels();
    run(source, target, sender, receiver);
}

fn transfer_messages(source: Task, target: Task, sender: Handle, receiver: Handle) {
    let allocation = memory::manager()
        .allocate(FrameOwner::User(1), FramePurpose::Data, 1)
        .unwrap();
    let allocation = memory::manager().allocate_arc(allocation).unwrap();
    let requests: [TransferRequest; 8] = core::array::from_fn(|_| TransferRequest {
        handle: source
            .insert(
                Object::FrameAllocation(allocation.clone()),
                Rights::TRANSFER | Rights::READ | Rights::WRITE,
            )
            .unwrap(),
        rights: Rights::READ,
    });
    let readonly = source.duplicate(sender, Rights::READ).unwrap();
    assert_eq!(
        source.try_send(readonly, message(73), &requests),
        Err(TaskError::Handle(HandleError::Denied))
    );
    source.close(readonly).unwrap();
    for (invalid, expected) in [
        (
            TransferRequest {
                handle: Handle::from_raw(0),
                rights: Rights::READ,
            },
            HandleError::BadHandle,
        ),
        (requests[0], HandleError::InvalidArgument),
        (
            TransferRequest {
                rights: Rights::EXECUTE,
                ..requests[1]
            },
            HandleError::Denied,
        ),
    ] {
        assert_eq!(
            source.try_send(sender, message(73), &[requests[0], invalid]),
            Err(TaskError::Handle(expected))
        );
    }
    assert_eq!(
        source.try_send(sender, message(73), &[requests[0]; 9]),
        Err(TaskError::Handle(HandleError::InvalidArgument))
    );
    assert_eq!(
        target.try_receive(receiver, 8),
        Err(TaskError::Channel(ChannelError::WouldBlock))
    );
    source.try_send(sender, message(73), &requests).unwrap();
    for request in requests {
        assert!(matches!(
            source.get(request.handle, ObjectType::FrameAllocation, Rights::READ),
            Err(TaskError::Handle(HandleError::BadHandle))
        ));
    }
    assert_eq!(
        target.try_receive(receiver, 7),
        Err(TaskError::Channel(ChannelError::BufferTooSmall))
    );
    let ObjectReference::Channel(reference) = target
        .get(receiver, ObjectType::Channel, Rights::READ)
        .unwrap()
    else {
        panic!("expected channel");
    };
    assert_eq!(reference.try_receive(), Err(ChannelError::BufferTooSmall));
    let received = target.receive(receiver, 8).unwrap();
    assert_eq!(received.message, message(73));
    assert_eq!(received.handles().len(), 8);
    for handle in received.handles() {
        let ObjectReference::FrameAllocation(frame) = target
            .get(*handle, ObjectType::FrameAllocation, Rights::READ)
            .unwrap()
        else {
            panic!("expected frame");
        };
        assert!(memory::HeapArc::ptr_eq(&frame, &allocation));
        assert!(matches!(
            target.get(*handle, ObjectType::FrameAllocation, Rights::WRITE),
            Err(TaskError::Handle(HandleError::Denied))
        ));
        target.close(*handle).unwrap();
    }
    source.close(sender).unwrap();
    assert_eq!(
        target.receive(receiver, 8),
        Err(TaskError::Channel(ChannelError::EndOfStream))
    );
    source.terminate(0);
    target.terminate(0);
}

fn transfer_endpoints() {
    for discard in [false, true] {
        let (source, target, sender, receiver) = task_channels();
        let (channel, peer) = Channel::pair().unwrap();
        check_endpoint_transfer(source, target, sender, receiver, channel, peer, discard);
    }
}

fn check_endpoint_transfer(
    source: Task,
    target: Task,
    sender: Handle,
    receiver: Handle,
    channel: Channel,
    peer: Channel,
    discard: bool,
) {
    let reference = channel.reference();
    let handle = source
        .insert(
            Object::Channel(channel),
            Rights::TRANSFER | Rights::READ | Rights::WRITE,
        )
        .unwrap();
    source
        .try_send(
            sender,
            message(19),
            &[TransferRequest {
                handle,
                rights: Rights::READ | Rights::WRITE,
            }],
        )
        .unwrap();
    assert_eq!(peer.try_receive(), Err(ChannelError::WouldBlock));
    peer.try_send(message(42)).unwrap();
    if discard {
        target.close(receiver).unwrap();
    } else {
        let received = target.try_receive(receiver, 1).unwrap();
        let handle = received.handles()[0];
        assert_eq!(target.try_receive(handle, 0).unwrap().message, message(42));
        target.close(handle).unwrap();
    }
    assert_eq!(reference.try_receive(), Err(ChannelError::Closed));
    assert_eq!(peer.try_receive(), Err(ChannelError::EndOfStream));
    source.terminate(0);
    target.terminate(0);
}

fn transfer_waits(source: Task, target: Task, sender: Handle, receiver: Handle) {
    let allocation = memory::manager()
        .allocate(FrameOwner::User(1), FramePurpose::Data, 1)
        .unwrap();
    let object = Object::FrameAllocation(memory::manager().allocate_arc(allocation).unwrap());
    receive_waiters(&source, &target, sender, receiver, object.clone());
    for tag in 0..32 {
        source.try_send(sender, message(tag), &[]).unwrap();
    }
    let original = source
        .insert(object.clone(), Rights::TRANSFER | Rights::READ)
        .unwrap();
    let worker = blocked_transfer(
        &source,
        sender,
        original,
        Err(TaskError::Handle(HandleError::BadHandle)),
    );
    let valid = source
        .insert(object.clone(), Rights::TRANSFER | Rights::READ)
        .unwrap();
    let next = blocked_transfer(&source, sender, valid, Ok(()));
    source.close(original).unwrap();
    let replacement = source
        .insert(object, Rights::TRANSFER | Rights::READ)
        .unwrap();
    assert_eq!(original.raw() as u32, replacement.raw() as u32);
    assert_ne!(original, replacement);
    target.try_receive(receiver, 0).unwrap();
    thread::yield_now().unwrap();
    assert_eq!(next.wait().unwrap(), 73);
    assert_eq!(worker.state(), ThreadState::Dead(73));
    let request = [TransferRequest {
        handle: replacement,
        rights: Rights::READ,
    }];
    assert_eq!(
        source.try_send(sender, message(73), &request),
        Err(TaskError::Channel(ChannelError::WouldBlock))
    );
    assert_eq!(
        source.send_until(
            sender,
            message(73),
            &request,
            hardware::timer().now_ns().unwrap()
        ),
        Err(TaskError::Channel(ChannelError::TimedOut))
    );
    assert!(
        source
            .get(replacement, ObjectType::FrameAllocation, Rights::READ)
            .is_ok()
    );
    let worker = blocked_transfer(
        &source,
        sender,
        replacement,
        Err(TaskError::Channel(ChannelError::Closed)),
    );
    source.terminate(19);
    thread::yield_now().unwrap();
    assert_eq!(worker.state(), ThreadState::Dead(73));
    for tag in 1..32 {
        assert_eq!(
            target.try_receive(receiver, 0).unwrap().message,
            message(tag)
        );
    }
    let received = target.try_receive(receiver, 1).unwrap();
    assert_eq!(received.message, message(73));
    assert_eq!(received.handles().len(), 1);
    assert_eq!(
        target.try_receive(receiver, 0),
        Err(TaskError::Channel(ChannelError::EndOfStream))
    );
    target.terminate(0);
}

fn blocked_transfer(
    source: &Task,
    sender: Handle,
    handle: Handle,
    expected: Result<(), TaskError>,
) -> Thread {
    *TRANSFER_SENDER.lock() = Some(TransferSender {
        task: source.clone(),
        sender,
        handle,
        expected,
    });
    let worker = Thread::new(
        |_| {
            let TransferSender {
                task,
                sender,
                handle,
                expected,
            } = TRANSFER_SENDER.lock().take().unwrap();
            assert_eq!(
                task.send(
                    sender,
                    message(73),
                    &[TransferRequest {
                        handle,
                        rights: Rights::READ
                    }]
                ),
                expected
            );
            73
        },
        0,
    )
    .unwrap();
    worker.start().unwrap();
    thread::yield_now().unwrap();
    assert_eq!(worker.state(), ThreadState::Blocked);
    worker
}

fn receive_waiters(source: &Task, target: &Task, sender: Handle, receiver: Handle, object: Object) {
    let mut workers = ArrayVec::<_, 2>::new();
    for capacity in 0..2 {
        *TRANSFER_RECEIVER.lock() = Some((target.clone(), receiver, capacity));
        let worker = Thread::new(
            |_| {
                let (task, receiver, capacity) = TRANSFER_RECEIVER.lock().take().unwrap();
                let received = task.receive(receiver, capacity);
                if capacity == 0 {
                    assert_eq!(
                        received,
                        Err(TaskError::Channel(ChannelError::BufferTooSmall))
                    );
                } else {
                    let received = received.unwrap();
                    assert_eq!(received.message, message(42));
                    assert_eq!(received.handles().len(), 1);
                    task.close(received.handles()[0]).unwrap();
                }
                73
            },
            0,
        )
        .unwrap();
        worker.start().unwrap();
        thread::yield_now().unwrap();
        assert_eq!(worker.state(), ThreadState::Blocked);
        workers.push(worker);
    }
    let handle = source
        .insert(object, Rights::TRANSFER | Rights::READ)
        .unwrap();
    source
        .try_send(
            sender,
            message(42),
            &[TransferRequest {
                handle,
                rights: Rights::READ,
            }],
        )
        .unwrap();
    for worker in workers {
        assert_eq!(worker.wait().unwrap(), 73);
    }
}

fn transfer_cycles() {
    let task = Task::new().unwrap();
    let (first, first_peer) = Channel::pair().unwrap();
    let (second, second_peer) = Channel::pair().unwrap();
    check_transfer_cycles(task, [first, first_peer, second, second_peer]);
}

fn check_transfer_cycles(task: Task, channels: [Channel; 4]) {
    let rights = Rights::TRANSFER | Rights::READ | Rights::WRITE;
    let handles = channels.map(|channel| task.insert(Object::Channel(channel), rights).unwrap());
    let send =
        |sender, handle| task.try_send(sender, message(73), &[TransferRequest { handle, rights }]);
    assert_eq!(
        send(handles[0], handles[0]),
        Err(TaskError::Channel(ChannelError::CyclicTransfer))
    );
    send(handles[1], handles[2]).unwrap();
    assert_eq!(
        send(handles[3], handles[0]),
        Err(TaskError::Channel(ChannelError::CyclicTransfer))
    );
    let received = task.try_receive(handles[0], 1).unwrap();
    send(handles[3], handles[1]).unwrap();
    task.close(received.handles()[0]).unwrap();
    assert_eq!(
        task.try_receive(handles[0], 0),
        Err(TaskError::Channel(ChannelError::EndOfStream))
    );
    task.terminate(0);
}

fn transfer_cleanup() {
    let task = Task::new().unwrap();
    let (sender, receiver) = Channel::pair().unwrap();
    let rights = Rights::TRANSFER | Rights::READ | Rights::WRITE;
    let mut sender = task.insert(Object::Channel(sender), rights).unwrap();
    let receiver = task.insert(Object::Channel(receiver), rights).unwrap();
    for tag in 0..128 {
        let (next, peer) = Channel::pair().unwrap();
        let next = task.insert(Object::Channel(next), rights).unwrap();
        let peer = task.insert(Object::Channel(peer), rights).unwrap();
        task.try_send(
            sender,
            message(tag),
            &[TransferRequest {
                handle: peer,
                rights,
            }],
        )
        .unwrap();
        sender = next;
    }
    task.close(receiver).unwrap();
    assert_eq!(
        task.try_send(sender, message(73), &[]),
        Err(TaskError::Channel(ChannelError::PeerClosed))
    );
    task.terminate(0);
}

fn receive_allocation_failure(source: Task, target: Task, sender: Handle, receiver: Handle) {
    let manager = memory::manager();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 1)
        .unwrap();
    let object = Object::FrameAllocation(manager.allocate_arc(allocation).unwrap());
    let requests: [TransferRequest; 8] = core::array::from_fn(|_| TransferRequest {
        handle: source
            .insert(object.clone(), Rights::TRANSFER | Rights::READ)
            .unwrap(),
        rights: Rights::READ,
    });
    source.try_send(sender, message(73), &requests).unwrap();
    let handle = target
        .insert(object, Rights::DUPLICATE | Rights::READ)
        .unwrap();
    manager.reclaim_heap();
    let exhausted = manager
        .allocate(
            FrameOwner::Kernel,
            FramePurpose::Data,
            manager.stats().kernel.free_frames,
        )
        .unwrap();
    loop {
        match target.duplicate(handle, Rights::DUPLICATE | Rights::READ) {
            Ok(_) => {}
            Err(error) => {
                assert_eq!(error, TaskError::Handle(HandleError::NoMemory));
                break;
            }
        }
    }
    assert_eq!(
        target.try_receive(receiver, 8),
        Err(TaskError::Handle(HandleError::NoMemory))
    );
    drop(exhausted);
    let received = target.try_receive(receiver, 8).unwrap();
    assert_eq!(received.message, message(73));
    assert_eq!(received.handles().len(), 8);
    source.terminate(0);
    target.terminate(0);
}

fn wait_for_deadline(worker: &Thread) {
    while worker.state() == ThreadState::Blocked {
        hardware::cpu().idle_until_interrupt();
    }
}

fn message(tag: u64) -> Message {
    let words = [tag, 1, 2, 3, 4, 5, 6, 7];
    Message::new(tag, &words[..tag as usize % 9]).unwrap()
}
