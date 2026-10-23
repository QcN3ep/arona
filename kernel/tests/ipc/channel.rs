// SPDX-License-Identifier: MPL-2.0

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

use kernel::{
    hardware,
    ipc::{Channel, ChannelError, ChannelReference, Message},
    memory,
    task::{HandleTable, Object, ObjectReference, ObjectType, Rights},
    thread::{Thread, ThreadError, ThreadState, scheduler},
};

static ENDPOINTS: Mutex<[Option<Channel>; 3]> = Mutex::new([const { None }; 3]);
static REFERENCES: Mutex<[Option<ChannelReference>; 2]> = Mutex::new([const { None }; 2]);
static RECEIVED: AtomicUsize = AtomicUsize::new(0);
static DEADLINE: AtomicU64 = AtomicU64::new(0);

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
    assert!(ENDPOINTS.lock().iter().all(Option::is_none));
    assert!(REFERENCES.lock().iter().all(Option::is_none));
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

fn wait_for_deadline(worker: &Thread) {
    while worker.state() == ThreadState::Blocked {
        hardware::cpu().idle_until_interrupt();
    }
}

fn message(tag: u64) -> Message {
    let words = [tag, 1, 2, 3, 4, 5, 6, 7];
    Message::new(tag, &words[..tag as usize % 9]).unwrap()
}
