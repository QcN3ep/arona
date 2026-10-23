// SPDX-License-Identifier: MPL-2.0

use allocator_api2::vec::Vec;
use spin::Mutex;

use crate::{
    hardware::{self, cpu::InterruptState},
    memory::{self, HeapArc, MemoryError},
    thread::{self, ThreadError, Wait, WaitOutcome, Waker},
};

const CAPACITY: usize = 32;
const MESSAGE_WORDS: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Message {
    pub tag: u64,
    words: [u64; MESSAGE_WORDS],
    word_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelError {
    InvalidMessage,
    WouldBlock,
    PeerClosed,
    EndOfStream,
    TimedOut,
    Memory(MemoryError),
    Thread(ThreadError),
}

/// # Safety
///
/// 操作及析构期间须保留内存管理器所需的内核映射。
#[derive(Clone)]
pub struct Channel(HeapArc<Endpoint>);

struct Endpoint {
    channel: HeapArc<Mutex<ChannelState>>,
    side: usize,
}

struct ChannelState {
    open: [bool; 2],
    directions: [Direction; 2],
}

struct Direction {
    messages: Vec<Option<Message>>,
    head: usize,
    length: usize,
    senders: WaitQueue,
    receivers: WaitQueue,
}

struct WaitQueue {
    entries: Vec<WaitEntry>,
    first: Option<usize>,
    last: Option<usize>,
    free: Option<usize>,
}

struct WaitEntry {
    waker: Option<Waker>,
    previous: Option<usize>,
    next: Option<usize>,
    notified: bool,
}

#[derive(Clone, Copy)]
enum Condition {
    Readable,
    Writable,
}

#[derive(Clone, Copy)]
enum WaitMode {
    NonBlocking,
    Blocking,
    Deadline(u64),
}

struct Registration {
    channel: HeapArc<Mutex<ChannelState>>,
    direction: usize,
    condition: Condition,
    index: usize,
    retrying: bool,
}

struct InterruptGuard(InterruptState);

impl Message {
    pub fn new(tag: u64, words: &[u64]) -> Result<Self, ChannelError> {
        if words.len() > MESSAGE_WORDS {
            return Err(ChannelError::InvalidMessage);
        }
        let mut message = Self {
            tag,
            words: [0; MESSAGE_WORDS],
            word_count: words.len(),
        };
        message.words[..words.len()].copy_from_slice(words);
        Ok(message)
    }

    pub fn words(&self) -> &[u64] {
        &self.words[..self.word_count]
    }
}

impl Channel {
    pub fn pair() -> Result<(Self, Self), ChannelError> {
        let _interrupts = InterruptGuard::new();
        let channel = memory::manager().allocate_arc(Mutex::new(ChannelState {
            open: [true; 2],
            directions: [Direction::new()?, Direction::new()?],
        }))?;
        let first = memory::manager().allocate_arc(Endpoint {
            channel: channel.clone(),
            side: 0,
        })?;
        let second = memory::manager().allocate_arc(Endpoint { channel, side: 1 })?;
        Ok((Self(first), Self(second)))
    }

    pub fn try_send(&self, message: Message) -> Result<(), ChannelError> {
        self.send_with(message, WaitMode::NonBlocking)
    }

    pub fn send(&self, message: Message) -> Result<(), ChannelError> {
        self.send_with(message, WaitMode::Blocking)
    }

    pub fn send_until(&self, message: Message, deadline_ns: u64) -> Result<(), ChannelError> {
        self.send_with(message, WaitMode::Deadline(deadline_ns))
    }

    pub fn try_receive(&self) -> Result<Message, ChannelError> {
        self.receive_with(WaitMode::NonBlocking)
    }

    pub fn receive(&self) -> Result<Message, ChannelError> {
        self.receive_with(WaitMode::Blocking)
    }

    pub fn receive_until(&self, deadline_ns: u64) -> Result<Message, ChannelError> {
        self.receive_with(WaitMode::Deadline(deadline_ns))
    }

    fn send_with(&self, message: Message, mode: WaitMode) -> Result<(), ChannelError> {
        loop {
            let (wait, mut registration) = {
                let _interrupts = InterruptGuard::new();
                let mut state = self.0.channel.lock();
                if !state.open[1 - self.0.side] {
                    return Err(ChannelError::PeerClosed);
                }
                let direction = &mut state.directions[self.0.side];
                if direction.length < CAPACITY {
                    let index = (direction.head + direction.length) % CAPACITY;
                    direction.messages[index] = Some(message);
                    direction.length += 1;
                    direction.receivers.wake_one();
                    return Ok(());
                }
                self.register(&mut state, self.0.side, Condition::Writable, mode)?
            };
            let result = mode.wait(wait);
            registration.retrying = result.is_ok();
            drop(registration);
            result?;
        }
    }

    fn receive_with(&self, mode: WaitMode) -> Result<Message, ChannelError> {
        let incoming = 1 - self.0.side;
        loop {
            let (wait, mut registration) = {
                let _interrupts = InterruptGuard::new();
                let mut state = self.0.channel.lock();
                let direction = &mut state.directions[incoming];
                if direction.length != 0 {
                    let message = direction.messages[direction.head].take().unwrap();
                    direction.head = (direction.head + 1) % CAPACITY;
                    direction.length -= 1;
                    direction.senders.wake_one();
                    return Ok(message);
                }
                if !state.open[incoming] {
                    return Err(ChannelError::EndOfStream);
                }
                self.register(&mut state, incoming, Condition::Readable, mode)?
            };
            let result = mode.wait(wait);
            registration.retrying = result.is_ok();
            drop(registration);
            result?;
        }
    }

    fn register(
        &self,
        state: &mut ChannelState,
        direction: usize,
        condition: Condition,
        mode: WaitMode,
    ) -> Result<(Wait, Registration), ChannelError> {
        let wait = mode.prepare()?;
        let index = state.waiters(direction, condition).insert(wait.waker())?;
        Ok((
            wait,
            Registration {
                channel: self.0.channel.clone(),
                direction,
                condition,
                index,
                retrying: false,
            },
        ))
    }
}

impl ChannelState {
    fn waiters(&mut self, direction: usize, condition: Condition) -> &mut WaitQueue {
        match condition {
            Condition::Readable => &mut self.directions[direction].receivers,
            Condition::Writable => &mut self.directions[direction].senders,
        }
    }

    fn ready(&self, direction: usize, condition: Condition) -> bool {
        match condition {
            Condition::Readable => self.directions[direction].length != 0 || !self.open[direction],
            Condition::Writable => {
                self.directions[direction].length < CAPACITY || !self.open[1 - direction]
            }
        }
    }
}

impl Direction {
    fn new() -> Result<Self, ChannelError> {
        let mut messages = Vec::new();
        messages
            .try_reserve_exact(CAPACITY)
            .map_err(|_| MemoryError::NoMemory)?;
        messages.resize_with(CAPACITY, || None);
        Ok(Self {
            messages,
            head: 0,
            length: 0,
            senders: WaitQueue::new(),
            receivers: WaitQueue::new(),
        })
    }
}

impl WaitQueue {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            first: None,
            last: None,
            free: None,
        }
    }

    fn insert(&mut self, waker: Waker) -> Result<usize, ChannelError> {
        let entry = WaitEntry {
            waker: Some(waker),
            previous: self.last,
            next: None,
            notified: false,
        };
        let index = if let Some(index) = self.free {
            self.free = self.entries[index].next;
            self.entries[index] = entry;
            index
        } else {
            self.entries
                .try_reserve(1)
                .map_err(|_| MemoryError::NoMemory)?;
            self.entries.push(entry);
            self.entries.len() - 1
        };
        if let Some(last) = self.last {
            self.entries[last].next = Some(index);
        } else {
            self.first = Some(index);
        }
        self.last = Some(index);
        Ok(index)
    }

    fn unlink(&mut self, index: usize) -> Option<Waker> {
        let entry = &mut self.entries[index];
        let waker = entry.waker.take()?;
        let previous = entry.previous.take();
        let next = entry.next.take();
        if let Some(previous) = previous {
            self.entries[previous].next = next;
        } else {
            self.first = next;
        }
        if let Some(next) = next {
            self.entries[next].previous = previous;
        } else {
            self.last = previous;
        }
        Some(waker)
    }

    fn remove(&mut self, index: usize) -> bool {
        self.unlink(index);
        let entry = &mut self.entries[index];
        let notified = core::mem::take(&mut entry.notified);
        entry.next = self.free;
        self.free = Some(index);
        notified
    }

    fn wake_one(&mut self) {
        while let Some(index) = self.first {
            let waker = self.unlink(index).unwrap();
            let notified = waker.wake();
            self.entries[index].notified = notified;
            if notified {
                break;
            }
        }
    }

    fn wake_all(&mut self) {
        while self.first.is_some() {
            self.wake_one();
        }
    }
}

impl WaitMode {
    fn prepare(self) -> Result<Wait, ChannelError> {
        match self {
            Self::NonBlocking => return Err(ChannelError::WouldBlock),
            Self::Deadline(deadline) => {
                if hardware::timer().now_ns().map_err(ThreadError::from)? >= deadline {
                    return Err(ChannelError::TimedOut);
                }
            }
            Self::Blocking => {}
        }
        thread::prepare_wait().map_err(Into::into)
    }

    fn wait(self, wait: Wait) -> Result<(), ChannelError> {
        match self {
            Self::NonBlocking => Err(ChannelError::WouldBlock),
            Self::Blocking => wait.wait().map_err(Into::into),
            Self::Deadline(deadline) => match wait.wait_until(deadline)? {
                WaitOutcome::Notified => Ok(()),
                WaitOutcome::TimedOut => Err(ChannelError::TimedOut),
            },
        }
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        let _interrupts = InterruptGuard::new();
        let mut state = self.channel.lock();
        let notified = state
            .waiters(self.direction, self.condition)
            .remove(self.index);
        // 被唤醒者退出时，将尚未使用的就绪通知交给下一等待者。
        if notified && !self.retrying && state.ready(self.direction, self.condition) {
            state.waiters(self.direction, self.condition).wake_one();
        }
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        let _interrupts = InterruptGuard::new();
        let mut state = self.channel.lock();
        state.open[self.side] = false;
        let incoming = &mut state.directions[1 - self.side];
        incoming.messages.fill(None);
        incoming.length = 0;
        for direction in &mut state.directions {
            direction.senders.wake_all();
            direction.receivers.wake_all();
        }
    }
}

impl InterruptGuard {
    fn new() -> Self {
        Self(hardware::cpu().disable_interrupts())
    }
}

impl Drop for InterruptGuard {
    fn drop(&mut self) {
        hardware::cpu().restore_interrupts(self.0);
    }
}

impl From<MemoryError> for ChannelError {
    fn from(error: MemoryError) -> Self {
        Self::Memory(error)
    }
}

impl From<ThreadError> for ChannelError {
    fn from(error: ThreadError) -> Self {
        Self::Thread(error)
    }
}
