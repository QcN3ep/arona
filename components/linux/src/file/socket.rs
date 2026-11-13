// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Shared socket ownership and independent readiness registrations.

use super::OpenFileDescription;
use alloc::{rc::Rc, vec::Vec};
use core::cell::{Cell, RefCell};
use runtime::ipc::{self, WaitItem, WaitMode};

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum Connection {
    Initial,
    Connecting,
    Connected,
}

pub(crate) struct Socket<'a> {
    pub kind: net::SocketKind,
    native: RefCell<Option<net::Socket<'a>>>,
    pub connection: Cell<Connection>,
    failed: Cell<bool>,
    pub received: RefCell<Option<Received>>,
    pub read_eof: Cell<bool>,
    pub receive_timeout: Cell<Option<u64>>,
    pub send_timeout: Cell<Option<u64>>,
}

pub(crate) struct Received {
    pub data: net::Data,
    pub offset: usize,
}

/// A syscall owns the native request only while an attempt is in flight.
pub(crate) struct Lease<'a> {
    description: Rc<OpenFileDescription<'a>>,
    pub native: Option<net::Socket<'a>>,
    pub recovering: bool,
}

#[derive(Default)]
pub(crate) struct Watch {
    registration: Option<net::PendingSubscription>,
    subscription: Option<net::Subscription>,
    snapshot: Option<net::PendingReadiness>,
    state: Option<net::Readiness>,
    refresh: Cell<bool>,
}

impl<'a> Socket<'a> {
    pub fn new(native: net::Socket<'a>, connected: bool) -> Self {
        Self {
            kind: native.kind(),
            native: RefCell::new(Some(native)),
            connection: Cell::new(if connected {
                Connection::Connected
            } else {
                Connection::Initial
            }),
            failed: Cell::new(false),
            received: RefCell::new(None),
            read_eof: Cell::new(false),
            receive_timeout: Cell::new(None),
            send_timeout: Cell::new(None),
        }
    }

    pub fn acquire(
        description: &Rc<OpenFileDescription<'a>>,
    ) -> Result<Option<Lease<'a>>, net::Error> {
        let state = Self::state(description);
        if state.failed.get() {
            return Err(net::Error::Unavailable);
        }
        Ok(state.native.borrow_mut().take().map(|native| Lease {
            recovering: native.has_pending(),
            native: Some(native),
            description: Rc::clone(description),
        }))
    }

    pub fn state<'b>(description: &'b OpenFileDescription<'a>) -> &'b Self {
        let OpenFileDescription::Socket { state, .. } = description else {
            unreachable!()
        };
        state
    }

    pub fn readable(&self) -> bool {
        self.received.borrow().is_some()
    }
}

impl Lease<'_> {
    pub fn fail(&mut self) {
        Socket::state(&self.description).failed.set(true);
        self.native = None;
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        // An abandoned attempt is drained by the next owner before issuing new I/O.
        *Socket::state(&self.description).native.borrow_mut() = self.native.take();
    }
}

impl Watch {
    pub fn invalidate(&mut self) {
        self.state = None;
    }

    pub fn poll(&mut self, socket: &Socket<'_>) -> Result<Option<net::Readiness>, net::Error> {
        if socket.failed.get() {
            return Err(net::Error::Unavailable);
        }
        if self.subscription.is_none() {
            if self.registration.is_none() {
                let native = socket.native.borrow();
                let Some(native) = native.as_ref() else {
                    return Ok(None);
                };
                self.registration = Some(native.start_subscribe()?);
            }
            match self.registration.as_mut().unwrap().try_complete() {
                Err(net::Error::WouldBlock) => return Ok(None),
                result => {
                    let (subscription, initial) = result?;
                    self.registration = None;
                    self.subscription = Some(subscription);
                    self.state = Some(initial);
                }
            }
        }
        let subscription = self.subscription.as_ref().unwrap();
        if self.refresh.replace(false) {
            self.state = None;
        }
        if self.snapshot.is_none() {
            match ipc::wait_many(
                &[
                    WaitItem::new(subscription.channel(), ipc::PEER_CLOSED),
                    WaitItem::new(subscription.event(), 1),
                ],
                WaitMode::Poll,
            ) {
                Ok(ready) if ready.index == 0 => return Err(net::Error::Unavailable),
                Ok(_) => self.state = None,
                Err(runtime::Error::Kernel(runtime::KernelError::WouldBlock)) => {}
                Err(error) => return Err(error.into()),
            }
            if self.state.is_none() {
                self.snapshot = Some(subscription.start_snapshot()?);
            }
        }
        if let Some(snapshot) = &mut self.snapshot {
            match snapshot.try_complete() {
                Err(net::Error::WouldBlock) => return Ok(None),
                result => {
                    self.state = Some(result?);
                    self.snapshot = None;
                }
            }
        }
        Ok(self.state)
    }

    pub fn waits<'a>(&'a self, items: &mut Vec<WaitItem<'a>>) {
        if let Some(registration) = &self.registration {
            items.push(WaitItem::new(registration.call(), ipc::READABLE));
        }
        if let Some(snapshot) = &self.snapshot {
            items.push(WaitItem::new(snapshot.call(), ipc::READABLE));
        }
        if let Some(subscription) = &self.subscription {
            // wait_many consumes Event notifications. Refresh after the dispatcher wakes.
            if self.snapshot.is_none() {
                self.refresh.set(true);
            }
            items.push(WaitItem::new(subscription.channel(), ipc::PEER_CLOSED));
            items.push(WaitItem::new(subscription.event(), 1));
        }
    }
}
