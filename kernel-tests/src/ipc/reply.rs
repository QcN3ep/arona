// SPDX-License-Identifier: MPL-2.0

use arrayvec::ArrayVec;
use spin::Mutex;

use kernel::{
    hardware,
    ipc::{Channel, ChannelError, Message},
    memory::{self, FrameOwner, FramePurpose},
    task::{Handle, HandleError, Object, ObjectType, Rights, Task, TaskError, TransferRequest},
    thread::{self, Thread, ThreadState},
};

static FIXTURE: Mutex<Option<Fixture>> = Mutex::new(None);
static REQUESTS: Mutex<ArrayVec<TransferRequest, 8>> = Mutex::new(ArrayVec::new_const());

#[derive(Clone)]
struct Fixture {
    client: Task,
    server: Task,
    sender: Handle,
    receiver: Handle,
}

impl Fixture {
    fn new() -> Self {
        let client = Task::new().unwrap();
        let server = Task::new().unwrap();
        let (sender, receiver) = Channel::pair().unwrap();
        let rights = Rights::DUPLICATE | Rights::TRANSFER | Rights::READ | Rights::WRITE;
        Self {
            sender: client.insert(Object::Channel(sender), rights).unwrap(),
            receiver: server.insert(Object::Channel(receiver), rights).unwrap(),
            client,
            server,
        }
    }

    fn start(&self, entry: thread::ThreadEntry, argument: usize) -> Thread {
        *FIXTURE.lock() = Some(self.clone());
        let caller = Thread::new(entry, argument).unwrap();
        caller.start().unwrap();
        thread::yield_now().unwrap();
        assert_eq!(caller.state(), ThreadState::Blocked);
        caller
    }
}

pub fn run() {
    super::run_thread(|_| {
        let fixture = Fixture::new();
        round_trip(&fixture)
    });
    super::run_thread(forwarding_and_close);
    super::run_thread(|_| {
        let fixture = Fixture::new();
        deadlines(&fixture)
    });
    super::run_thread(|_| {
        let fixture = Fixture::new();
        reply_under_pressure(&fixture)
    });
    FIXTURE.lock().take();
    assert!(REQUESTS.lock().is_empty());
}

fn message(tag: u64) -> Message {
    Message::new(tag, &[tag]).unwrap()
}

fn invoke(_: usize) -> usize {
    let fixture = FIXTURE.lock().as_ref().unwrap().clone();
    let requests = core::mem::take(&mut *REQUESTS.lock());
    let response = fixture
        .client
        .call(fixture.sender, message(42), &requests)
        .unwrap();
    assert_eq!(response.message, message(73));
    assert!(!response.has_reply());
    assert_eq!(response.handles().len(), 8);
    for handle in response.handles() {
        assert!(
            fixture
                .client
                .get(*handle, ObjectType::FrameAllocation, Rights::READ)
                .is_ok()
        );
        assert!(matches!(
            fixture
                .client
                .get(*handle, ObjectType::FrameAllocation, Rights::WRITE),
            Err(TaskError::Handle(HandleError::Denied))
        ));
        fixture.client.close(*handle).unwrap();
    }
    0
}

fn round_trip(fixture: &Fixture) -> usize {
    let allocation = memory::manager()
        .allocate(FrameOwner::User(1), FramePurpose::Data, 1)
        .unwrap();
    let allocation = memory::manager().allocate_arc(allocation).unwrap();
    for _ in 0..7 {
        let handle = fixture
            .client
            .insert(
                Object::FrameAllocation(allocation.clone()),
                Rights::TRANSFER | Rights::READ,
            )
            .unwrap();
        REQUESTS.lock().push(TransferRequest {
            handle,
            rights: Rights::READ,
        });
    }
    let caller = fixture.start(invoke, 0);
    assert_eq!(
        fixture.server.try_receive(fixture.receiver, 7),
        Err(TaskError::Channel(ChannelError::BufferTooSmall))
    );
    let request = fixture.server.receive(fixture.receiver, 8).unwrap();
    assert!(request.has_reply());
    assert_eq!(request.message, message(42));
    assert_eq!(request.handles().len(), 8);
    let reply = request.handles()[0];
    assert_eq!(
        fixture.server.duplicate(reply, Rights::WRITE),
        Err(TaskError::Handle(HandleError::Denied))
    );
    let child = fixture.server.create_child().unwrap();
    assert_eq!(
        child.install_handle(
            &fixture.server,
            TransferRequest {
                handle: reply,
                rights: Rights::WRITE
            }
        ),
        Err(TaskError::Handle(HandleError::Denied))
    );
    assert_eq!(
        fixture.server.reply(
            reply,
            message(73),
            &[TransferRequest {
                handle: Handle::from_raw(0),
                rights: Rights::READ
            }]
        ),
        Err(TaskError::Handle(HandleError::BadHandle))
    );
    for handle in &request.handles()[1..] {
        fixture.server.close(*handle).unwrap();
    }
    let requests: [_; 8] = core::array::from_fn(|_| TransferRequest {
        handle: fixture
            .server
            .insert(
                Object::FrameAllocation(allocation.clone()),
                Rights::TRANSFER | Rights::READ,
            )
            .unwrap(),
        rights: Rights::READ,
    });
    fixture.server.reply(reply, message(73), &requests).unwrap();
    assert_eq!(
        fixture.server.reply(reply, message(73), &[]),
        Err(TaskError::Channel(ChannelError::InvalidState))
    );
    fixture.server.close(reply).unwrap();
    assert_eq!(caller.wait().unwrap(), 0);
    FIXTURE.lock().take();
    0
}

fn await_result(expected: usize) -> usize {
    let fixture = FIXTURE.lock().as_ref().unwrap().clone();
    let result = fixture.client.call(fixture.sender, message(42), &[]);
    if expected == 0 {
        let response = result.unwrap();
        assert_eq!(response.message, message(73));
        assert!(response.handles().is_empty());
    } else {
        assert_eq!(result, Err(TaskError::Channel(ChannelError::ServerGone)));
    }
    0
}

fn forwarding_and_close(_: usize) -> usize {
    for close in 0..4 {
        let fixture = Fixture::new();
        let caller = fixture.start(await_result, usize::from(close != 0));
        complete_call(&fixture, close);
        assert_eq!(caller.wait().unwrap(), 0);
        FIXTURE.lock().take();
    }
    0
}

fn complete_call(fixture: &Fixture, close: usize) {
    if close == 1 {
        fixture.server.close(fixture.receiver).unwrap();
    } else {
        let request = fixture.server.receive(fixture.receiver, 8).unwrap();
        let reply = request.handles()[0];
        match close {
            2 => fixture.server.close(reply).unwrap(),
            3 => fixture.server.terminate(1),
            _ => {
                let (sender, receiver) = Channel::pair().unwrap();
                let sender = fixture
                    .server
                    .insert(Object::Channel(sender), Rights::WRITE)
                    .unwrap();
                let target = Task::new().unwrap();
                let receiver = target
                    .insert(Object::Channel(receiver), Rights::READ)
                    .unwrap();
                fixture
                    .server
                    .try_send(
                        sender,
                        message(19),
                        &[TransferRequest {
                            handle: reply,
                            rights: Rights::WRITE,
                        }],
                    )
                    .unwrap();
                fixture.server.close(fixture.receiver).unwrap();
                let forwarded = target.receive(receiver, 8).unwrap();
                assert!(!forwarded.has_reply());
                target
                    .reply(forwarded.handles()[0], message(73), &[])
                    .unwrap();
            }
        }
    }
}

fn timeout(delay: usize) -> usize {
    let fixture = FIXTURE.lock().as_ref().unwrap().clone();
    let deadline = hardware::timer().now_ns().unwrap() + delay as u64;
    let requests = core::mem::take(&mut *REQUESTS.lock());
    assert_eq!(
        fixture
            .client
            .call_until(fixture.sender, message(42), &requests, deadline),
        Err(TaskError::Channel(ChannelError::TimedOut))
    );
    0
}

fn deadlines(fixture: &Fixture) -> usize {
    let event = kernel::ipc::Event::new().unwrap();
    let handle = fixture
        .client
        .insert(Object::Event(event), Rights::TRANSFER | Rights::WAIT)
        .unwrap();
    let request = TransferRequest {
        handle,
        rights: Rights::WAIT,
    };
    assert_eq!(
        fixture
            .client
            .call_until(fixture.sender, message(42), &[request], 0),
        Err(TaskError::Channel(ChannelError::TimedOut))
    );
    assert!(
        fixture
            .client
            .get(handle, ObjectType::Event, Rights::WAIT)
            .is_ok()
    );
    assert_eq!(
        fixture.server.try_receive(fixture.receiver, 8),
        Err(TaskError::Channel(ChannelError::WouldBlock))
    );
    for _ in 0..32 {
        fixture
            .client
            .try_send(fixture.sender, message(19), &[])
            .unwrap();
    }
    REQUESTS.lock().push(request);
    let caller = fixture.start(timeout, 100_000_000);
    assert_eq!(caller.wait().unwrap(), 0);
    assert!(
        fixture
            .client
            .get(handle, ObjectType::Event, Rights::WAIT)
            .is_ok()
    );
    for _ in 0..32 {
        assert_eq!(
            fixture
                .server
                .try_receive(fixture.receiver, 8)
                .unwrap()
                .message,
            message(19)
        );
    }
    REQUESTS.lock().push(request);
    let caller = fixture.start(timeout, 100_000_000);
    let limit = hardware::timer().now_ns().unwrap() + 200_000_000;
    while hardware::timer().now_ns().unwrap() < limit {
        core::hint::spin_loop();
    }
    let queued = fixture.server.receive(fixture.receiver, 8).unwrap();
    assert!(queued.has_reply());
    assert_eq!(queued.handles().len(), 2);
    assert!(matches!(
        fixture.client.get(handle, ObjectType::Event, Rights::WAIT),
        Err(TaskError::Handle(HandleError::BadHandle))
    ));
    assert_eq!(
        fixture.server.reply(queued.handles()[0], message(73), &[]),
        Err(TaskError::Channel(ChannelError::InvalidState))
    );
    fixture.server.close(queued.handles()[0]).unwrap();
    assert_eq!(caller.wait().unwrap(), 0);
    FIXTURE.lock().take();
    0
}

fn reply_under_pressure(fixture: &Fixture) -> usize {
    let caller = fixture.start(invoke, 0);
    let request = fixture.server.receive(fixture.receiver, 8).unwrap();
    let manager = memory::manager();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 1)
        .unwrap();
    let allocation = manager.allocate_arc(allocation).unwrap();
    let object = fixture
        .client
        .insert(
            Object::FrameAllocation(allocation.clone()),
            Rights::DUPLICATE | Rights::READ,
        )
        .unwrap();
    let requests: [_; 8] = core::array::from_fn(|_| TransferRequest {
        handle: fixture
            .server
            .insert(
                Object::FrameAllocation(allocation.clone()),
                Rights::TRANSFER | Rights::READ,
            )
            .unwrap(),
        rights: Rights::READ,
    });
    manager.reclaim_heap();
    let exhausted = manager
        .allocate(
            FrameOwner::Kernel,
            FramePurpose::Data,
            manager.stats().kernel.free_frames,
        )
        .unwrap();
    loop {
        match fixture
            .client
            .duplicate(object, Rights::DUPLICATE | Rights::READ)
        {
            Ok(_) => {}
            Err(error) => {
                assert_eq!(error, TaskError::Handle(HandleError::NoMemory));
                break;
            }
        }
    }
    fixture
        .server
        .reply(request.handles()[0], message(73), &requests)
        .unwrap();
    drop(exhausted);
    assert_eq!(caller.wait().unwrap(), 0);
    FIXTURE.lock().take();
    0
}
