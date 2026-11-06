// SPDX-License-Identifier: MPL-2.0

use alloc::vec::Vec;
use kernel::{
    ipc::{
        CallReference, Channel, ChannelError, Event, Message, READABLE, WaitItem, WaitManyError,
    },
    memory::{self, FrameOwner, FramePurpose},
    task::{Handle, HandleError, Object, ObjectType, Rights, Task, TaskError, TransferRequest},
    thread::{self, Thread, ThreadState},
};
use spin::Mutex;

static WAITING: Mutex<Option<(Task, Handle)>> = Mutex::new(None);

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
        let rights = ObjectType::Channel.supported_rights();
        Self {
            sender: client.insert(Object::Channel(sender), rights).unwrap(),
            receiver: server.insert(Object::Channel(receiver), rights).unwrap(),
            client,
            server,
        }
    }

    fn submit(&self, tag: u64) -> (Handle, CallReference) {
        let call = self
            .client
            .start_call(self.sender, message(tag), &[])
            .unwrap();
        let reference = call.reference();
        let handle = self
            .client
            .insert(Object::Call(call), ObjectType::Call.supported_rights())
            .unwrap();
        (handle, reference)
    }

    fn request(&self) -> Handle {
        let request = self.server.try_receive(self.receiver, 8).unwrap();
        assert!(request.has_reply());
        request.handles()[0]
    }
}

pub fn run() {
    super::run_thread(multiplex);
    super::run_thread(cancellation);
    super::run_thread(backpressure);
    super::run_thread(allocation_failure);
    assert!(WAITING.lock().is_none());
}

fn message(tag: u64) -> Message {
    Message::new(tag, &[tag]).unwrap()
}

fn multiplex(_: usize) -> usize {
    let fixture = Fixture::new();
    let (first, first_ref) = fixture.submit(1);
    let (second, second_ref) = fixture.submit(2);
    let items = [first, second].map(|handle| WaitItem {
        handle,
        condition_mask: READABLE,
    });
    assert_eq!(
        first_ref.try_receive(),
        Err(TaskError::Channel(ChannelError::WouldBlock))
    );
    assert_eq!(
        fixture.client.try_wait_many(&items),
        Err(TaskError::Wait(WaitManyError::WouldBlock))
    );
    assert_eq!(
        fixture.client.duplicate(first, Rights::READ),
        Err(TaskError::Handle(HandleError::Denied))
    );
    let child = fixture.client.create_child().unwrap();
    assert_eq!(
        child.install_handle(
            &fixture.client,
            TransferRequest {
                handle: first,
                rights: Rights::READ
            }
        ),
        Err(TaskError::Handle(HandleError::Denied))
    );
    let first_reply = fixture.request();
    let second_reply = fixture.request();
    let event = fixture
        .server
        .insert(
            Object::Event(Event::new().unwrap()),
            Rights::TRANSFER | Rights::WAIT,
        )
        .unwrap();
    fixture
        .server
        .reply(
            second_reply,
            message(22),
            &[TransferRequest {
                handle: event,
                rights: Rights::WAIT,
            }],
        )
        .unwrap();
    assert_eq!(second_ref.cancel(), Err(ChannelError::InvalidState));
    assert_eq!(fixture.client.try_wait_many(&items).unwrap().index, 1);
    assert_eq!(fixture.client.try_wait_many(&items).unwrap().index, 1);
    let response = second_ref.try_receive().unwrap();
    assert_eq!(response.message, message(22));
    assert_eq!(response.handles().len(), 1);
    fixture
        .client
        .get(response.handles()[0], ObjectType::Event, Rights::WAIT)
        .unwrap();
    fixture.client.close(response.handles()[0]).unwrap();
    assert_eq!(
        second_ref.try_receive(),
        Err(TaskError::Channel(ChannelError::InvalidState))
    );
    *WAITING.lock() = Some((fixture.client.clone(), first));
    let waiter = Thread::new(wait_reply, 0).unwrap();
    waiter.start().unwrap();
    thread::yield_now().unwrap();
    assert_eq!(waiter.state(), ThreadState::Blocked);
    fixture.server.reply(first_reply, message(11), &[]).unwrap();
    assert_eq!(waiter.wait().unwrap(), 0);
    assert_eq!(first_ref.try_receive().unwrap().message, message(11));
    fixture.server.close(first_reply).unwrap();
    fixture.server.close(second_reply).unwrap();
    fixture.client.close(first).unwrap();
    fixture.client.close(second).unwrap();
    WAITING.lock().take();
    0
}

fn wait_reply(_: usize) -> usize {
    let (task, handle) = WAITING.lock().as_ref().unwrap().clone();
    assert_eq!(
        task.wait_many(&[WaitItem {
            handle,
            condition_mask: READABLE
        }])
        .unwrap()
        .ready,
        READABLE
    );
    0
}

fn cancellation(_: usize) -> usize {
    let fixture = Fixture::new();
    let (handle, call) = fixture.submit(1);
    assert_eq!(
        fixture.client.wait_many_until(
            &[WaitItem {
                handle,
                condition_mask: READABLE
            }],
            0
        ),
        Err(TaskError::Wait(WaitManyError::TimedOut))
    );
    call.cancel().unwrap();
    assert_eq!(
        call.try_receive(),
        Err(TaskError::Channel(ChannelError::Cancelled))
    );
    let reply = fixture.request();
    let event = fixture
        .server
        .insert(
            Object::Event(Event::new().unwrap()),
            Rights::TRANSFER | Rights::WAIT,
        )
        .unwrap();
    assert_eq!(
        fixture.server.reply(
            reply,
            message(3),
            &[TransferRequest {
                handle: event,
                rights: Rights::WAIT
            }]
        ),
        Err(TaskError::Channel(ChannelError::InvalidState))
    );
    fixture
        .server
        .get(event, ObjectType::Event, Rights::WAIT)
        .unwrap();
    fixture.server.close(reply).unwrap();
    fixture.client.close(handle).unwrap();
    let (handle, call) = fixture.submit(2);
    let reply = fixture.request();
    fixture
        .server
        .reply(
            reply,
            message(4),
            &[TransferRequest {
                handle: event,
                rights: Rights::WAIT,
            }],
        )
        .unwrap();
    fixture.client.close(handle).unwrap();
    assert_eq!(
        call.try_receive(),
        Err(TaskError::Channel(ChannelError::Closed))
    );
    fixture.server.close(reply).unwrap();
    let (handle, call) = fixture.submit(3);
    fixture.client.close(handle).unwrap();
    let reply = fixture.request();
    assert!(fixture.server.reply(reply, message(5), &[]).is_err());
    fixture.server.close(reply).unwrap();
    assert_eq!(
        call.try_receive(),
        Err(TaskError::Channel(ChannelError::Closed))
    );
    let (handle, call) = fixture.submit(4);
    fixture.server.close(fixture.receiver).unwrap();
    assert_eq!(
        call.try_receive(),
        Err(TaskError::Channel(ChannelError::ServerGone))
    );
    fixture.client.close(handle).unwrap();
    0
}

fn backpressure(_: usize) -> usize {
    let fixture = Fixture::new();
    for _ in 0..32 {
        fixture
            .client
            .try_send(fixture.sender, message(0), &[])
            .unwrap();
    }
    let event = fixture
        .client
        .insert(
            Object::Event(Event::new().unwrap()),
            Rights::TRANSFER | Rights::WAIT,
        )
        .unwrap();
    assert!(matches!(
        fixture.client.start_call(
            fixture.sender,
            message(1),
            &[TransferRequest {
                handle: event,
                rights: Rights::WAIT
            }]
        ),
        Err(TaskError::Channel(ChannelError::WouldBlock))
    ));
    fixture
        .client
        .get(event, ObjectType::Event, Rights::WAIT)
        .unwrap();
    fixture.server.try_receive(fixture.receiver, 8).unwrap();
    let call = fixture
        .client
        .start_call(
            fixture.sender,
            message(2),
            &[TransferRequest {
                handle: event,
                rights: Rights::WAIT,
            }],
        )
        .unwrap();
    assert!(matches!(
        fixture.client.get(event, ObjectType::Event, Rights::WAIT),
        Err(TaskError::Handle(HandleError::BadHandle))
    ));
    fixture.server.terminate(0);
    assert_eq!(
        call.try_receive(),
        Err(TaskError::Channel(ChannelError::ServerGone))
    );
    0
}

fn allocation_failure(_: usize) -> usize {
    let fixture = Fixture::new();
    let mut calls = Vec::with_capacity(32);
    let before = kernel::ipc::active_replies();
    for _ in 0..32 {
        calls.push(
            fixture
                .client
                .start_call(fixture.sender, message(0), &[])
                .unwrap(),
        );
        fixture.server.close(fixture.request()).unwrap();
    }
    calls.clear();
    assert_eq!(kernel::ipc::active_replies(), before);
    let manager = memory::manager();
    manager.reclaim_heap();
    let exhausted = manager
        .allocate(
            FrameOwner::Kernel,
            FramePurpose::Data,
            manager.stats().kernel.free_frames,
        )
        .unwrap();
    loop {
        match fixture.client.start_call(fixture.sender, message(1), &[]) {
            Ok(call) => {
                assert!(calls.len() < calls.capacity());
                calls.push(call);
                fixture.server.close(fixture.request()).unwrap();
            }
            Err(error) => {
                assert_eq!(abi::Error::from(error), abi::Error::NoMemory);
                break;
            }
        }
    }
    assert_eq!(kernel::ipc::active_replies(), before + calls.len());
    assert_eq!(
        fixture.server.try_receive(fixture.receiver, 8),
        Err(TaskError::Channel(ChannelError::WouldBlock))
    );
    drop(exhausted);
    calls.clear();
    assert_eq!(kernel::ipc::active_replies(), before);
    0
}
