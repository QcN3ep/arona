// SPDX-License-Identifier: MPL-2.0

use core::ops::Deref;

use alloc::{collections::VecDeque, vec::Vec};
use arrayvec::ArrayVec;
use spin::Mutex;

use crate::{
    hardware::{self, cpu::InterruptGuard},
    memory::{self, HeapArc, MemoryError},
    task::{Handle, HandleEntry, Object},
    thread::{self, ThreadError, Wait, WaitOutcome, Waker},
};

use super::wait::{Observer, Observers, WaitMode};

const CAPACITY: usize = 32;
const MESSAGE_WORDS: usize = 8;
pub(crate) const MESSAGE_HANDLES: usize = 8;
type MessageHandles = Vec<HandleEntry>;

static CLEANUP: Mutex<ChannelCleanup> = Mutex::new(ChannelCleanup {
    first: None,
    active: false,
});

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Message {
    pub tag: u64,
    words: [u64; MESSAGE_WORDS],
    word_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceivedMessage {
    pub message: Message,
    handles: ArrayVec<Handle, MESSAGE_HANDLES>,
    has_reply: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelError {
    InvalidMessage,
    BufferTooSmall,
    CyclicTransfer,
    Closed,
    WouldBlock,
    PeerClosed,
    EndOfStream,
    TimedOut,
    InvalidState,
    ServerGone,
    Memory(MemoryError),
    Thread(ThreadError),
}

/// # Safety
///
/// 操作及析构期间须保留内存管理器所需的内核映射。
pub struct Channel {
    reference: ChannelReference,
}

/// 保持端点存储存活，不计入开放引用。
///
/// # Safety
///
/// 操作及析构期间须保留内存管理器所需的内核映射。
#[derive(Clone)]
pub struct ChannelReference {
    channel: HeapArc<Mutex<ChannelState>>,
    side: usize,
}

struct ChannelState {
    credits: [usize; 2],
    directions: [Direction; 2],
    observers: [Observers; 2],
    cleanup_next: Option<HeapArc<Mutex<Self>>>,
    queued_for_cleanup: bool,
}

struct ChannelCleanup {
    first: Option<HeapArc<Mutex<ChannelState>>>,
    active: bool,
}

struct QueuedMessage {
    message: Message,
    handles: MessageHandles,
    has_reply: bool,
}

struct Direction {
    messages: VecDeque<QueuedMessage>,
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

struct Registration {
    channel: HeapArc<Mutex<ChannelState>>,
    direction: usize,
    condition: Condition,
    index: usize,
    retrying: bool,
}

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

impl ReceivedMessage {
    pub(crate) fn new(
        message: Message,
        handles: ArrayVec<Handle, MESSAGE_HANDLES>,
        has_reply: bool,
    ) -> Self {
        Self {
            message,
            handles,
            has_reply,
        }
    }

    pub fn handles(&self) -> &[Handle] {
        &self.handles
    }

    pub fn has_reply(&self) -> bool {
        self.has_reply
    }
}

impl Channel {
    pub fn pair() -> Result<(Self, Self), ChannelError> {
        let _interrupts = InterruptGuard::new();
        // 分开构造状态与分配队列，降低内核栈峰值。
        let channel = ChannelState::allocate()?;
        for direction in &mut channel.lock().directions {
            direction
                .messages
                .try_reserve_exact(CAPACITY)
                .map_err(|_| MemoryError::NoMemory)?;
        }
        Ok((
            Self {
                reference: ChannelReference {
                    channel: channel.clone(),
                    side: 0,
                },
            },
            Self {
                reference: ChannelReference { channel, side: 1 },
            },
        ))
    }

    pub fn reference(&self) -> ChannelReference {
        self.reference.clone()
    }
}

impl Clone for Channel {
    fn clone(&self) -> Self {
        let _interrupts = InterruptGuard::new();
        self.reference.channel.lock().credits[self.reference.side] += 1;
        Self {
            reference: self.reference(),
        }
    }
}

impl Deref for Channel {
    type Target = ChannelReference;

    fn deref(&self) -> &Self::Target {
        &self.reference
    }
}

impl ChannelReference {
    pub(crate) fn same(&self, other: &Self) -> bool {
        HeapArc::ptr_eq(&self.channel, &other.channel) && self.side == other.side
    }

    pub fn try_send(&self, message: Message) -> Result<(), ChannelError> {
        self.send_with(message, WaitMode::NonBlocking, false, || Ok(Vec::new()))
    }

    pub fn send(&self, message: Message) -> Result<(), ChannelError> {
        self.send_with(message, WaitMode::Blocking, false, || Ok(Vec::new()))
    }

    pub fn send_until(&self, message: Message, deadline_ns: u64) -> Result<(), ChannelError> {
        self.send_with(message, WaitMode::Deadline(deadline_ns), false, || {
            Ok(Vec::new())
        })
    }

    pub fn try_receive(&self) -> Result<Message, ChannelError> {
        self.receive_without_handles(WaitMode::NonBlocking)
    }

    pub fn receive(&self) -> Result<Message, ChannelError> {
        self.receive_without_handles(WaitMode::Blocking)
    }

    pub fn receive_until(&self, deadline_ns: u64) -> Result<Message, ChannelError> {
        self.receive_without_handles(WaitMode::Deadline(deadline_ns))
    }

    pub(crate) fn send_with<E: From<ChannelError>>(
        &self,
        message: Message,
        mode: WaitMode,
        has_reply: bool,
        mut transfer: impl FnMut() -> Result<MessageHandles, E>,
    ) -> Result<(), E> {
        loop {
            let (wait, mut registration) = {
                let _interrupts = InterruptGuard::new();
                let mut state = self.channel.lock();
                if state.credits[self.side] == 0 {
                    return Err(ChannelError::Closed.into());
                }
                if state.credits[1 - self.side] == 0 {
                    return Err(ChannelError::PeerClosed.into());
                }
                let direction = &mut state.directions[self.side];
                if direction.messages.len() < CAPACITY {
                    // 句柄转移依次取得 TASKS、句柄表和 Channel 的锁。
                    drop(state);
                    let handles = match transfer() {
                        Ok(handles) => handles,
                        Err(error) => {
                            self.channel.lock().directions[self.side].senders.wake_one();
                            return Err(error);
                        }
                    };
                    let mut state = self.channel.lock();
                    let direction = &mut state.directions[self.side];
                    direction.messages.push_back(QueuedMessage {
                        message,
                        handles,
                        has_reply,
                    });
                    direction.receivers.wake_one();
                    drop(state);
                    self.notify_observers();
                    return Ok(());
                }
                self.register(&mut state, self.side, Condition::Writable, mode)?
            };
            let result = mode.wait(wait);
            registration.retrying = result.is_ok();
            drop(registration);
            result?;
        }
    }

    fn receive_without_handles(&self, mode: WaitMode) -> Result<Message, ChannelError> {
        self.receive_with(mode, 0, |_| Ok(ArrayVec::new()))
            .map(|received| received.message)
    }

    pub(crate) fn receive_with<E: From<ChannelError>>(
        &self,
        mode: WaitMode,
        handle_capacity: usize,
        mut install: impl FnMut(&mut MessageHandles) -> Result<ArrayVec<Handle, MESSAGE_HANDLES>, E>,
    ) -> Result<ReceivedMessage, E> {
        let incoming = 1 - self.side;
        loop {
            let (wait, mut registration) = {
                let _interrupts = InterruptGuard::new();
                let mut state = self.channel.lock();
                if state.credits[self.side] == 0 {
                    return Err(ChannelError::Closed.into());
                }
                let direction = &mut state.directions[incoming];
                if let Some(queued) = direction.messages.front() {
                    if queued.handles.len() > handle_capacity {
                        direction.receivers.wake_one();
                        return Err(ChannelError::BufferTooSmall.into());
                    }
                    let mut queued = direction.messages.pop_front().unwrap();
                    drop(state);
                    let handles = match install(&mut queued.handles) {
                        Ok(handles) => handles,
                        Err(error) => {
                            let mut state = self.channel.lock();
                            let direction = &mut state.directions[incoming];
                            direction.messages.push_front(queued);
                            direction.receivers.wake_one();
                            return Err(error);
                        }
                    };
                    let message = queued.message;
                    let has_reply = queued.has_reply;
                    drop(queued);
                    let mut state = self.channel.lock();
                    let direction = &mut state.directions[incoming];
                    direction.senders.wake_one();
                    drop(state);
                    self.notify_observers();
                    return Ok(ReceivedMessage::new(message, handles, has_reply));
                }
                if state.credits[incoming] == 0 {
                    return Err(ChannelError::EndOfStream.into());
                }
                self.register(&mut state, incoming, Condition::Readable, mode)?
            };
            let result = mode.wait(wait);
            registration.retrying = result.is_ok();
            drop(registration);
            result?;
        }
    }

    pub(crate) fn check_transfer(&self, objects: &[&Object]) -> Result<(), ChannelError> {
        let mut channels = Vec::<ChannelReference>::new();
        for object in objects {
            if let Object::Channel(channel) = object {
                self.visit(&mut channels, channel)?;
            }
        }
        let mut cursor = 0;
        while cursor < channels.len() {
            let channel = channels[cursor].channel.clone();
            let state = channel.lock();
            for direction in &state.directions {
                for message in &direction.messages {
                    for entry in &message.handles {
                        if let Object::Channel(channel) = &entry.object {
                            self.visit(&mut channels, channel)?;
                        }
                    }
                }
            }
            cursor += 1;
        }
        Ok(())
    }

    fn visit(
        &self,
        channels: &mut Vec<ChannelReference>,
        channel: &ChannelReference,
    ) -> Result<(), ChannelError> {
        if HeapArc::ptr_eq(&self.channel, &channel.channel) {
            return Err(ChannelError::CyclicTransfer);
        }
        if !channels
            .iter()
            .any(|visited| HeapArc::ptr_eq(&visited.channel, &channel.channel))
        {
            channels.try_reserve(1).map_err(|_| MemoryError::NoMemory)?;
            channels.push(channel.clone());
        }
        Ok(())
    }

    pub(crate) fn readiness(&self, conditions: u64) -> Result<u64, ChannelError> {
        let _interrupts = InterruptGuard::new();
        let state = self.channel.lock();
        if state.credits[self.side] == 0 {
            return Err(ChannelError::Closed);
        }
        let incoming = 1 - self.side;
        let direction = &state.directions[incoming];
        let mut ready = 0;
        if conditions & super::wait::READABLE != 0 && !direction.messages.is_empty() {
            ready |= super::wait::READABLE;
        }
        if conditions & super::wait::WRITABLE != 0
            && state.credits[incoming] != 0
            && state.directions[self.side].messages.len() < CAPACITY
        {
            ready |= super::wait::WRITABLE;
        }
        if conditions & super::wait::PEER_CLOSED != 0 && state.credits[incoming] == 0 {
            ready |= super::wait::PEER_CLOSED;
        }
        Ok(ready)
    }

    pub(crate) fn register_observer(&self, observer: Observer) -> Result<(), MemoryError> {
        self.channel.lock().observers[self.side].insert(observer)
    }

    pub(crate) fn unregister_observer(&self, observer: &Observer) {
        self.channel.lock().observers[self.side].remove(observer);
    }

    fn notify_observers(&self) {
        for side in 0..2 {
            let mut index = 0;
            loop {
                let observer = self.channel.lock().observers[side].get(index);
                let Some(observer) = observer else { break };
                observer.notify();
                index += 1;
            }
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
                channel: self.channel.clone(),
                direction,
                condition,
                index,
                retrying: false,
            },
        ))
    }
}

impl ChannelState {
    fn allocate() -> Result<HeapArc<Mutex<Self>>, ChannelError> {
        Ok(memory::manager().allocate_arc(Mutex::new(Self {
            credits: [1; 2],
            directions: [Direction::new(), Direction::new()],
            observers: [Observers::new(), Observers::new()],
            cleanup_next: None,
            queued_for_cleanup: false,
        }))?)
    }

    fn discard(channel: &HeapArc<Mutex<Self>>) {
        {
            let mut cleanup = CLEANUP.lock();
            let mut state = channel.lock();
            if !state.queued_for_cleanup {
                state.cleanup_next = cleanup.first.take();
                state.queued_for_cleanup = true;
                cleanup.first = Some(channel.clone());
            }
            if cleanup.active {
                return;
            }
            cleanup.active = true;
        }
        // 迭代释放消息中的端点，避免析构链耗尽内核栈。
        loop {
            let channel = {
                let mut cleanup = CLEANUP.lock();
                let Some(channel) = cleanup.first.take() else {
                    cleanup.active = false;
                    return;
                };
                let mut state = channel.lock();
                cleanup.first = state.cleanup_next.take();
                state.queued_for_cleanup = false;
                drop(state);
                channel
            };
            loop {
                let entry = channel.lock().discard_entry();
                if entry.is_none() {
                    break;
                }
                drop(entry);
            }
        }
    }

    fn discard_entry(&mut self) -> Option<HandleEntry> {
        for side in 0..2 {
            if self.credits[1 - side] != 0 {
                continue;
            }
            let messages = &mut self.directions[side].messages;
            while let Some(message) = messages.front_mut() {
                if let Some(entry) = message.handles.pop() {
                    return Some(entry);
                }
                drop(messages.drain(..1));
            }
        }
        None
    }

    fn waiters(&mut self, direction: usize, condition: Condition) -> &mut WaitQueue {
        match condition {
            Condition::Readable => &mut self.directions[direction].receivers,
            Condition::Writable => &mut self.directions[direction].senders,
        }
    }

    fn ready(&self, direction: usize, condition: Condition) -> bool {
        if self.credits.contains(&0) {
            return true;
        }
        match condition {
            Condition::Readable => !self.directions[direction].messages.is_empty(),
            Condition::Writable => self.directions[direction].messages.len() < CAPACITY,
        }
    }
}

impl Direction {
    const fn new() -> Self {
        Self {
            messages: VecDeque::new(),
            senders: WaitQueue::new(),
            receivers: WaitQueue::new(),
        }
    }
}

impl WaitQueue {
    const fn new() -> Self {
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

impl Drop for Channel {
    fn drop(&mut self) {
        let _interrupts = InterruptGuard::new();
        let endpoint = &self.reference;
        let discard = {
            let mut state = endpoint.channel.lock();
            state.credits[endpoint.side] -= 1;
            if state.credits[endpoint.side] != 0 {
                return;
            }
            for direction in &mut state.directions {
                direction.senders.wake_all();
                direction.receivers.wake_all();
            }
            !state.directions[1 - endpoint.side].messages.is_empty()
        };
        endpoint.notify_observers();
        if discard {
            ChannelState::discard(&endpoint.channel);
        }
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
