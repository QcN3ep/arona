// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Local Unix socket pairs with independent stream or datagram directions.

use super::OpenFileDescription;
use alloc::{collections::VecDeque, rc::Rc, vec::Vec};
use core::{
    cell::{Cell, RefCell},
    sync::atomic::{AtomicU64, Ordering},
};

const CAPACITY: usize = 64 * 1024;
pub(crate) const MAX_DATAGRAM: usize = 65507;

static NEXT_PAIR: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum Kind {
    Stream,
    Datagram,
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub(crate) struct Readiness {
    pub readable: bool,
    pub writable: bool,
    pub hangup: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum SendError {
    TooLarge,
    WouldBlock,
    NoMemory,
}

/// One endpoint's open state, including its inherited I/O timeouts.
pub(crate) struct SocketPair {
    endpoint: Endpoint,
    pub receive_timeout: Cell<Option<u64>>,
    pub send_timeout: Cell<Option<u64>>,
}

struct Endpoint {
    pair: Rc<RefCell<Pair>>,
    index: usize,
}

struct Pair {
    id: u64,
    kind: Kind,
    endpoints: [Direction; 2],
}

struct Direction {
    stream: VecDeque<u8>,
    messages: VecDeque<Vec<u8>>,
    used: usize,
    capacity: usize,
    read_shutdown: bool,
    write_shutdown: bool,
    closed: bool,
}

impl SocketPair {
    pub fn pair(kind: Kind) -> Result<(Self, Self), ()> {
        let mut id = NEXT_PAIR.load(Ordering::Relaxed);
        loop {
            let next = id.checked_add(1).ok_or(())?;
            match NEXT_PAIR.compare_exchange_weak(id, next, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => break,
                Err(current) => id = current,
            }
        }
        let pair = Rc::new(RefCell::new(Pair {
            id,
            kind,
            endpoints: [Direction::new(kind)?, Direction::new(kind)?],
        }));
        Ok((
            Self {
                endpoint: Endpoint {
                    pair: Rc::clone(&pair),
                    index: 0,
                },
                receive_timeout: Cell::new(None),
                send_timeout: Cell::new(None),
            },
            Self {
                endpoint: Endpoint { pair, index: 1 },
                receive_timeout: Cell::new(None),
                send_timeout: Cell::new(None),
            },
        ))
    }

    pub fn state<'b, 'a>(description: &'b OpenFileDescription<'a>) -> &'b Self {
        let OpenFileDescription::SocketPair { state, .. } = description else {
            unreachable!()
        };
        state
    }

    pub fn kind(&self) -> Kind {
        self.endpoint.pair.borrow().kind
    }

    pub fn inode(&self) -> u64 {
        self.endpoint.pair.borrow().id
    }

    pub fn capacity(&self) -> usize {
        self.endpoint
            .pair
            .borrow()
            .local(self.endpoint.index)
            .capacity
    }

    pub fn has_data(&self) -> bool {
        let pair = self.endpoint.pair.borrow();
        pair.local(self.endpoint.index).has_data(pair.kind)
    }

    pub fn available(&self) -> usize {
        let pair = self.endpoint.pair.borrow();
        pair.local(self.endpoint.index).available(pair.kind)
    }

    /// Space in the peer's receive queue, which bounds this endpoint's sends.
    pub fn free(&self) -> usize {
        let pair = self.endpoint.pair.borrow();
        pair.peer(self.endpoint.index).free()
    }

    /// The local read side has been shut down, so reads return zero.
    pub fn read_closed(&self) -> bool {
        let pair = self.endpoint.pair.borrow();
        pair.local(self.endpoint.index).read_shutdown
    }

    /// The peer can no longer write, so reads report end of file after queued data.
    pub fn peer_closed(&self) -> bool {
        let pair = self.endpoint.pair.borrow();
        let peer = pair.peer(self.endpoint.index);
        peer.write_shutdown || peer.closed
    }

    /// The local write side or the peer's read side is shut down.
    pub fn write_broken(&self) -> bool {
        let pair = self.endpoint.pair.borrow();
        let local = pair.local(self.endpoint.index);
        let peer = pair.peer(self.endpoint.index);
        local.write_shutdown || peer.read_shutdown || peer.closed
    }

    pub fn shutdown(&self, direction: net::Shutdown) {
        let mut pair = self.endpoint.pair.borrow_mut();
        let local = pair.local_mut(self.endpoint.index);
        match direction {
            net::Shutdown::Read => local.read_shutdown = true,
            net::Shutdown::Write => local.write_shutdown = true,
            net::Shutdown::Both => {
                local.read_shutdown = true;
                local.write_shutdown = true;
            }
        }
    }

    pub fn readiness(&self) -> Readiness {
        let pair = self.endpoint.pair.borrow();
        let local = pair.local(self.endpoint.index);
        let peer = pair.peer(self.endpoint.index);
        Readiness {
            readable: local.read_shutdown
                || local.has_data(pair.kind)
                || peer.write_shutdown
                || peer.closed,
            writable: !local.write_shutdown
                && !peer.read_shutdown
                && !peer.closed
                && peer.free() != 0,
            hangup: peer.closed || (peer.read_shutdown && peer.write_shutdown),
        }
    }

    pub fn snapshot(&self, count: usize) -> Vec<u8> {
        let pair = self.endpoint.pair.borrow();
        let local = pair.local(self.endpoint.index);
        match pair.kind {
            Kind::Stream => local.stream.iter().copied().take(count).collect(),
            Kind::Datagram => local.messages.front().map_or_else(Vec::new, |message| {
                message[..count.min(message.len())].to_vec()
            }),
        }
    }

    /// Remove stream bytes or the whole front datagram.
    pub fn consume(&self, count: usize) {
        let mut pair = self.endpoint.pair.borrow_mut();
        let kind = pair.kind;
        let local = pair.local_mut(self.endpoint.index);
        match kind {
            Kind::Stream => {
                let count = count.min(local.stream.len());
                local.stream.drain(..count);
                local.used -= count;
            }
            Kind::Datagram => {
                if let Some(message) = local.messages.pop_front() {
                    local.used -= message.len().max(1);
                }
            }
        }
    }

    pub fn send_stream(&self, bytes: &[u8]) -> usize {
        let index = self.endpoint.index;
        let mut pair = self.endpoint.pair.borrow_mut();
        let peer = pair.peer_mut(index);
        let count = peer.free().min(bytes.len());
        peer.stream.extend(&bytes[..count]);
        peer.used += count;
        count
    }

    pub fn send_message(&self, bytes: &mut Vec<u8>) -> Result<(), SendError> {
        let index = self.endpoint.index;
        let mut pair = self.endpoint.pair.borrow_mut();
        let peer = pair.peer_mut(index);
        if bytes.len() > MAX_DATAGRAM {
            return Err(SendError::TooLarge);
        }
        let size = bytes.len().max(1);
        if peer.free() < size {
            return Err(SendError::WouldBlock);
        }
        peer.messages
            .try_reserve(1)
            .map_err(|_| SendError::NoMemory)?;
        peer.used += size;
        peer.messages.push_back(core::mem::take(bytes));
        Ok(())
    }
}

impl Pair {
    fn local(&self, index: usize) -> &Direction {
        &self.endpoints[index]
    }

    fn local_mut(&mut self, index: usize) -> &mut Direction {
        &mut self.endpoints[index]
    }

    fn peer(&self, index: usize) -> &Direction {
        &self.endpoints[1 - index]
    }

    fn peer_mut(&mut self, index: usize) -> &mut Direction {
        &mut self.endpoints[1 - index]
    }
}

impl Direction {
    fn new(kind: Kind) -> Result<Self, ()> {
        let mut stream = VecDeque::new();
        if kind == Kind::Stream {
            stream.try_reserve_exact(CAPACITY).map_err(|_| ())?;
        }
        Ok(Self {
            stream,
            messages: VecDeque::new(),
            used: 0,
            capacity: CAPACITY,
            read_shutdown: false,
            write_shutdown: false,
            closed: false,
        })
    }

    fn has_data(&self, kind: Kind) -> bool {
        match kind {
            Kind::Stream => !self.stream.is_empty(),
            Kind::Datagram => !self.messages.is_empty(),
        }
    }

    fn available(&self, kind: Kind) -> usize {
        match kind {
            Kind::Stream => self.stream.len(),
            Kind::Datagram => self.messages.front().map_or(0, Vec::len),
        }
    }

    fn free(&self) -> usize {
        self.capacity - self.used
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        let mut pair = self.pair.borrow_mut();
        pair.local_mut(self.index).closed = true;
    }
}
