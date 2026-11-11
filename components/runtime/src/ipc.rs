// SPDX-License-Identifier: MPL-2.0

use crate::{
    AnyHandle, Call, Channel, Error, Event, Handle, Object, Reply, Rights, Task, Timer, call,
    thread,
};
pub use abi::ipc::{EXITED, MESSAGE_HANDLES, MESSAGE_WORDS, PEER_CLOSED, READABLE, WRITABLE};
use abi::method;
use alloc::vec::Vec;
use arrayvec::ArrayVec;
use core::marker::PhantomData;

#[derive(Clone, Copy, Debug)]
pub enum WaitMode {
    Poll,
    Forever,
    Until(u64),
}
impl WaitMode {
    fn raw(self) -> Result<u64, Error> {
        match self {
            Self::Poll => Ok(0),
            Self::Forever => Ok(u64::MAX),
            Self::Until(0 | u64::MAX) => Err(Error::Kernel(abi::Error::InvalidArgument)),
            Self::Until(deadline) => Ok(deadline),
        }
    }
}
#[derive(Debug)]
pub struct Transfer {
    pub handle: AnyHandle,
    pub rights: Rights,
}
#[derive(Debug)]
pub struct Message {
    pub tag: u64,
    words: ArrayVec<u64, MESSAGE_WORDS>,
    transfers: ArrayVec<Transfer, MESSAGE_HANDLES>,
}
impl Message {
    pub fn new(tag: u64, words: &[u64]) -> Result<Self, Error> {
        let mut data = ArrayVec::new();
        data.try_extend_from_slice(words)
            .map_err(|_| Error::Kernel(abi::Error::InvalidArgument))?;
        Ok(Self {
            tag,
            words: data,
            transfers: ArrayVec::new(),
        })
    }
    pub fn words(&self) -> &[u64] {
        &self.words
    }
    pub fn transfers(&self) -> &[Transfer] {
        &self.transfers
    }
    pub fn push(&mut self, transfer: Transfer) -> Result<(), Transfer> {
        self.transfers
            .try_push(transfer)
            .map_err(|error| error.element())
    }
    pub fn pop(&mut self) -> Option<Transfer> {
        self.transfers.pop()
    }
    fn encode(&self) -> Result<[u8; abi::ipc::Message::SIZE], Error> {
        let mut wire = abi::ipc::Message::new(self.tag);
        wire.word_count = self.words.len();
        wire.words[..wire.word_count].copy_from_slice(&self.words);
        wire.handle_count = self.transfers.len();
        for (index, transfer) in self.transfers.iter().enumerate() {
            wire.handles[index] = Some(abi::ipc::HandleDescriptor {
                handle: transfer.handle.raw(),
                kind: transfer.handle.kind(),
                rights: transfer.rights,
            });
        }
        wire.encode().map_err(Error::Kernel)
    }
    fn commit(&mut self) {
        for transfer in self.transfers.drain(..) {
            let _ = transfer.handle.into_raw();
        }
    }
}
#[derive(Debug)]
pub struct ReceivedMessage {
    pub tag: u64,
    words: ArrayVec<u64, MESSAGE_WORDS>,
    handles: ArrayVec<AnyHandle, MESSAGE_HANDLES>,
    has_reply: bool,
}
impl ReceivedMessage {
    pub fn words(&self) -> &[u64] {
        &self.words
    }
    pub fn handles(&self) -> &[AnyHandle] {
        &self.handles
    }
    pub fn take_handle(&mut self, index: usize) -> Option<AnyHandle> {
        if index >= self.handles.len() {
            return None;
        }
        if index == 0 {
            self.has_reply = false;
        }
        Some(self.handles.remove(index))
    }
    pub fn take_reply(&mut self) -> Option<Handle<Reply>> {
        if !self.has_reply {
            return None;
        }
        self.take_handle(0)?.downcast().ok()
    }
    fn decode(bytes: &[u8; abi::ipc::Message::SIZE]) -> Result<Self, Error> {
        let wire = abi::ipc::Message::decode(bytes).map_err(|_| Error::Protocol)?;
        let handles = wire
            .handles
            .into_iter()
            .take(wire.handle_count)
            .flatten()
            .map(AnyHandle::received)
            .collect();
        Ok(Self {
            tag: wire.tag,
            words: wire.words.into_iter().take(wire.word_count).collect(),
            handles,
            has_reply: wire.has_reply,
        })
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CallError {
    NotSubmitted(Error),
    Submitted(Error),
}

pub struct PendingCall<T, S = (), E = Error> {
    call: Handle<Call>,
    state: Option<S>,
    complete: fn(S, ReceivedMessage) -> Result<T, E>,
}

impl<T, S, E: From<Error>> PendingCall<T, S, E> {
    pub fn new(
        call: Handle<Call>,
        state: S,
        complete: fn(S, ReceivedMessage) -> Result<T, E>,
    ) -> Self {
        Self {
            call,
            state: Some(state),
            complete,
        }
    }

    pub fn call(&self) -> &Handle<Call> {
        &self.call
    }

    pub fn cancel(&self) -> Result<(), E> {
        self.call.cancel().map_err(E::from)
    }

    /// After a timeout, this call can still be waited on or canceled.
    pub fn wait(&mut self, mode: WaitMode) -> Result<T, E> {
        wait_many(&[WaitItem::new(&self.call, READABLE)], mode).map_err(E::from)?;
        self.try_complete()
    }

    pub fn try_complete(&mut self) -> Result<T, E> {
        let response = self.call.try_receive().map_err(E::from)?;
        let state = self
            .state
            .take()
            .ok_or_else(|| E::from(Error::Kernel(abi::Error::InvalidState)))?;
        (self.complete)(state, response)
    }
}
impl Handle<Task> {
    pub fn channel(
        &self,
        first: Rights,
        second: Rights,
    ) -> Result<(Handle<Channel>, Handle<Channel>), Error> {
        let [first, second] = self.borrow().invoke(
            method::task::CHANNEL,
            [first.bits(), second.bits(), 0, 0, 0],
        )?;
        // SAFETY: A successful CHANNEL delivers two exclusive Channel handles.
        Ok(unsafe {
            (
                Handle::from_raw(abi::Handle::from_raw(first)),
                Handle::from_raw(abi::Handle::from_raw(second)),
            )
        })
    }
    pub fn event(&self, rights: Rights) -> Result<Handle<Event>, Error> {
        let [raw, _] = self
            .borrow()
            .invoke(method::task::EVENT, [rights.bits(), 0, 0, 0, 0])?;
        // SAFETY: A successful EVENT delivers an exclusive Event handle.
        Ok(unsafe { Handle::from_raw(abi::Handle::from_raw(raw)) })
    }
    pub fn timer(
        &self,
        event: &Handle<Event>,
        mask: u64,
        rights: Rights,
    ) -> Result<Handle<Timer>, Error> {
        let [raw, _] = self.borrow().invoke(
            method::task::TIMER,
            [event.borrow().raw().raw(), mask, rights.bits(), 0, 0],
        )?;
        // SAFETY: A successful TIMER delivers an exclusive Timer handle.
        Ok(unsafe { Handle::from_raw(abi::Handle::from_raw(raw)) })
    }
}
impl Handle<Channel> {
    pub fn start_call(&self, message: &mut Message) -> Result<Handle<Call>, Error> {
        let bytes = message.encode()?;
        let [raw, _] = self.borrow().invoke(
            method::channel::START_CALL,
            [bytes.as_ptr() as u64, 0, 0, 0, 0],
        )?;
        message.commit();
        // SAFETY: A successful START_CALL delivers an exclusive Call handle.
        Ok(unsafe { Handle::from_raw(abi::Handle::from_raw(raw)) })
    }

    pub fn send(&self, message: &mut Message, mode: WaitMode) -> Result<(), Error> {
        let bytes = message.encode()?;
        self.borrow().invoke(
            method::channel::SEND,
            [bytes.as_ptr() as u64, mode.raw()?, 0, 0, 0],
        )?;
        message.commit();
        Ok(())
    }
    pub fn receive(&self, mode: WaitMode) -> Result<ReceivedMessage, Error> {
        let mut bytes = [0; abi::ipc::Message::SIZE];
        self.borrow().invoke(
            method::channel::RECEIVE,
            [
                bytes.as_mut_ptr() as u64,
                MESSAGE_HANDLES as u64,
                mode.raw()?,
                0,
                0,
            ],
        )?;
        ReceivedMessage::decode(&bytes)
    }
    pub fn call(
        &self,
        message: &mut Message,
        deadline_ns: Option<u64>,
    ) -> Result<ReceivedMessage, CallError> {
        let bytes = match message.encode() {
            Ok(bytes) => bytes,
            Err(error) => return Err(CallError::NotSubmitted(error)),
        };
        let mut output = [0; abi::ipc::Message::SIZE];
        let result = call::invoke_return(
            self.borrow().raw(),
            method::channel::CALL,
            [
                bytes.as_ptr() as u64,
                output.as_mut_ptr() as u64,
                deadline_ns.unwrap_or(u64::MAX),
                0,
                0,
            ],
        );
        match result.values[1] {
            0 if result.status != 0 => {
                Err(CallError::NotSubmitted(call::check(result).unwrap_err()))
            }
            1 => {
                message.commit();
                call::check(result).map_err(CallError::Submitted)?;
                ReceivedMessage::decode(&output).map_err(CallError::Submitted)
            }
            _ => {
                message.commit();
                Err(CallError::Submitted(Error::Protocol))
            }
        }
    }
}
impl Handle<Call> {
    pub fn try_receive(&self) -> Result<ReceivedMessage, Error> {
        let mut output = [0; abi::ipc::Message::SIZE];
        self.borrow().invoke(
            method::call::RECEIVE,
            [output.as_mut_ptr() as u64, 0, 0, 0, 0],
        )?;
        ReceivedMessage::decode(&output)
    }

    /// Cancel a wait that has not completed; operations already performed by the server are not undone.
    pub fn cancel(&self) -> Result<(), Error> {
        self.borrow()
            .invoke(method::call::CANCEL, [0; 5])
            .map(|_| ())
    }
}

impl Handle<Reply> {
    pub fn send(&self, message: &mut Message) -> Result<(), Error> {
        let bytes = message.encode()?;
        self.borrow()
            .invoke(method::reply::SEND, [bytes.as_ptr() as u64, 0, 0, 0, 0])?;
        message.commit();
        Ok(())
    }
}
impl Handle<Event> {
    pub fn signal(&self, mask: u64) -> Result<(), Error> {
        self.borrow()
            .invoke(method::event::SIGNAL, [mask, 0, 0, 0, 0])
            .map(|_| ())
    }
}
impl Handle<Timer> {
    pub fn arm(&self, deadline_ns: u64) -> Result<(), Error> {
        self.borrow()
            .invoke(method::timer::ARM, [deadline_ns, 0, 0, 0, 0])
            .map(|_| ())
    }
    pub fn cancel(&self) -> Result<(), Error> {
        self.borrow()
            .invoke(method::timer::CANCEL, [0; 5])
            .map(|_| ())
    }
}

pub struct WaitItem<'a> {
    raw: abi::ipc::WaitItem,
    _handle: PhantomData<&'a ()>,
}
impl<'a> WaitItem<'a> {
    pub fn new<T: Object>(handle: &'a Handle<T>, conditions: u64) -> Self {
        Self {
            raw: abi::ipc::WaitItem {
                handle: handle.borrow().raw(),
                conditions,
            },
            _handle: PhantomData,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WaitResult {
    pub index: usize,
    pub ready: u64,
}
pub fn wait_many(items: &[WaitItem<'_>], mode: WaitMode) -> Result<WaitResult, Error> {
    if items.is_empty() {
        return Err(Error::Kernel(abi::Error::InvalidArgument));
    }
    let length = items
        .len()
        .checked_mul(abi::ipc::WaitItem::SIZE)
        .ok_or(Error::Kernel(abi::Error::InvalidArgument))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| Error::Kernel(abi::Error::NoMemory))?;
    for item in items {
        bytes.extend_from_slice(&item.raw.encode());
    }
    let [index, ready] = thread::current().invoke(
        method::thread::WAIT_MANY,
        [bytes.as_ptr() as u64, items.len() as u64, mode.raw()?, 0, 0],
    )?;
    if index >= items.len() as u64 {
        return Err(Error::Protocol);
    }
    Ok(WaitResult {
        index: index as usize,
        ready,
    })
}
