// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Socket attempts, blocking retries, and committed result delivery.

use super::{Output, error, install, message::Receive};
use crate::{
    file::{
        O_NONBLOCK, OpenFileDescription,
        socket::{Connection, Lease, Received, Socket, Watch},
    },
    process::Process,
    syscall::{abi::*, wait::Deadline},
};
use alloc::{boxed::Box, rc::Rc, vec::Vec};
use runtime::ipc::{self, WaitItem};

pub(in crate::syscall) enum Pending<'a> {
    Create {
        native: Option<net::Socket<'a>>,
        flags: u32,
    },
    Operation(Box<Operation<'a>>),
    Value(Output, usize),
    Bytes(Output, Vec<u8>),
    Done,
}

pub(in crate::syscall) struct Operation<'a> {
    description: Rc<OpenFileDescription<'a>>,
    action: Action,
    lease: Option<Lease<'a>>,
    watch: Watch,
    waiting: bool,
    started: bool,
    progress: bool,
    nonblocking: bool,
    deadline: Deadline,
}

pub(super) enum Action {
    Bind(net::Endpoint),
    Listen(u8),
    Connect {
        endpoint: net::Endpoint,
        finishing: bool,
    },
    Disconnect,
    Accept {
        peer: Option<Output>,
        flags: u32,
    },
    Send {
        bytes: Vec<u8>,
        peer: Option<net::Endpoint>,
    },
    Receive(Receive),
    Endpoint {
        peer: bool,
        output: Output,
    },
    GetOption {
        name: Option<net::OptionName>,
        output: Output,
        checking: bool,
    },
    SetOption(net::OptionName, u32),
    Shutdown(net::Shutdown),
}

impl<'a> Pending<'a> {
    pub fn take_progress(&mut self) -> bool {
        match self {
            Self::Operation(operation) => core::mem::take(&mut operation.progress),
            _ => false,
        }
    }

    pub fn poll(&mut self, process: &mut Process<'a>) -> Option<SyscallResult> {
        match self {
            Self::Value(output, value) => Some(output.value(process, *value)),
            Self::Bytes(output, bytes) => Some(output.bytes(process, bytes)),
            Self::Done => Some(Ok(0)),
            Self::Operation(operation) => operation.poll(process).transpose(),
            Self::Create { native, flags } => match native.as_mut().unwrap().try_complete() {
                Err(net::Error::WouldBlock) if native.as_ref().unwrap().has_pending() => None,
                Ok(net::Completion::Done) => Some(install(
                    process,
                    native.take().unwrap(),
                    *flags,
                    false,
                    [None; 2],
                )),
                Err(reason) => Some(Err(error(reason))),
                Ok(_) => Some(Err(EIO)),
            },
        }
    }

    pub fn interrupt(&self) -> Option<(bool, SyscallResult)> {
        let Self::Operation(operation) = self else {
            return None;
        };
        if operation
            .lease
            .as_ref()
            .is_some_and(|lease| lease.native.as_ref().is_some_and(net::Socket::has_pending))
        {
            return None;
        }
        if !operation.action.waitable() {
            return None;
        }
        Some((
            !matches!(operation.action, Action::Connect { .. })
                && operation.deadline.deadline().is_none(),
            Err(EINTR),
        ))
    }

    pub fn waits<'b>(&'b self, items: &mut Vec<WaitItem<'b>>) {
        match self {
            Self::Create {
                native: Some(native),
                ..
            } => {
                if let Some(call) = native.call() {
                    items.push(WaitItem::new(call, ipc::READABLE));
                }
            }
            Self::Operation(operation) => {
                if operation.lease.is_none() {
                    operation.deadline.waits(items);
                }
                operation.watch.waits(items);
                if let Some(call) = operation
                    .lease
                    .as_ref()
                    .and_then(|lease| lease.native.as_ref())
                    .and_then(net::Socket::call)
                {
                    items.push(WaitItem::new(call, ipc::READABLE));
                }
            }
            _ => {}
        }
    }
}

impl Action {
    fn waitable(&self) -> bool {
        matches!(
            self,
            Self::Connect { .. } | Self::Accept { .. } | Self::Send { .. } | Self::Receive(_)
        )
    }

    fn ready(&self, ready: net::Readiness) -> bool {
        ready.error
            || ready.closed
            || match self {
                Self::Connect { .. } | Self::Send { .. } => ready.writable || ready.write_closed,
                _ => ready.readable || ready.read_closed,
            }
    }

    fn request(&self, kind: net::SocketKind) -> net::Request<'_> {
        match self {
            Self::Bind(endpoint) => net::Request::Bind(*endpoint),
            Self::Disconnect => net::Request::Disconnect,
            Self::Listen(backlog) => net::Request::Listen(*backlog),
            Self::GetOption { checking: true, .. } => net::Request::PeerEndpoint,
            Self::Connect {
                endpoint,
                finishing: false,
            } => net::Request::Connect(*endpoint),
            Self::Connect {
                finishing: true, ..
            }
            | Self::GetOption { name: None, .. } => net::Request::TakeError,
            Self::Accept { .. } => net::Request::Accept,
            Self::Send { bytes, peer } => match peer {
                Some(peer) => net::Request::SendTo(bytes, *peer),
                None => net::Request::Send(bytes),
            },
            Self::Receive(receive) => receive.request(kind),
            Self::Endpoint { peer, .. } => {
                if *peer {
                    net::Request::PeerEndpoint
                } else {
                    net::Request::LocalEndpoint
                }
            }
            Self::GetOption {
                name: Some(name), ..
            } => net::Request::GetOption(*name),
            Self::SetOption(name, value) => net::Request::SetOption(*name, *value),
            Self::Shutdown(direction) => net::Request::Shutdown(*direction),
        }
    }
}

impl<'a> Operation<'a> {
    pub(super) fn new(
        description: Rc<OpenFileDescription<'a>>,
        action: Action,
        nonblocking: bool,
        process: &Process<'_>,
    ) -> Result<Self, Errno> {
        let socket = Socket::state(&description);
        let duration = if nonblocking || description.flags() & O_NONBLOCK != 0 {
            None
        } else {
            match action {
                Action::Receive(_) | Action::Accept { .. } => socket.receive_timeout.get(),
                Action::Send { .. } | Action::Connect { .. } => socket.send_timeout.get(),
                _ => None,
            }
        };
        Ok(Self {
            description,
            action,
            lease: None,
            watch: Watch::default(),
            waiting: false,
            started: false,
            progress: false,
            nonblocking,
            deadline: Deadline::new(process.supervisor, duration)?,
        })
    }

    fn poll(&mut self, process: &mut Process<'a>) -> Result<Option<usize>, Errno> {
        loop {
            if let Action::Receive(receive) = &self.action
                && let Some(count) = receive.buffered(process, Socket::state(&self.description))?
            {
                return Ok(Some(count));
            }
            if self.lease.is_none() {
                if self.deadline.expired()? {
                    return Err(if matches!(self.action, Action::Connect { .. }) {
                        EINPROGRESS
                    } else {
                        EAGAIN
                    });
                }
                if self.action.waitable() {
                    let Some(ready) = self
                        .watch
                        .poll(Socket::state(&self.description))
                        .map_err(error)?
                    else {
                        return Ok(None);
                    };
                    if self.waiting && !self.action.ready(ready) {
                        return Ok(None);
                    }
                }
                self.lease = Socket::acquire(&self.description).map_err(error)?;
                if self.lease.is_none() {
                    return Ok(None);
                }
            }
            let Some(result) = self.attempt()? else {
                return Ok(None);
            };
            let nonblocking = self.nonblocking || self.description.flags() & O_NONBLOCK != 0;
            match result {
                Err(net::Error::WouldBlock) if self.action.waitable() && !nonblocking => {
                    self.waiting = true;
                    self.watch.invalidate();
                }
                Err(net::Error::InProgress) if matches!(self.action, Action::Connect { .. }) => {
                    if nonblocking {
                        return Err(EINPROGRESS);
                    }
                    if let Action::Connect { finishing, .. } = &mut self.action {
                        *finishing = true;
                    }
                    self.waiting = true;
                    self.watch.invalidate();
                }
                result => {
                    if let Some(count) = self.finish(process, result)? {
                        return Ok(Some(count));
                    }
                }
            }
        }
    }

    fn attempt(&mut self) -> Result<Option<Result<net::Completion<'a>, net::Error>>, Errno> {
        let lease = self.lease.as_mut().unwrap();
        let native = lease.native.as_mut().unwrap();
        if lease.recovering {
            let result = native.try_complete();
            if matches!(result, Err(net::Error::WouldBlock)) && native.has_pending() {
                return Ok(None);
            }
            if native.has_pending() {
                lease.fail();
                return Err(EIO);
            }
            // The previous thread exited. Drop its committed result before reusing the socket.
            drop(result);
            lease.recovering = false;
        }
        if !self.started {
            if Socket::state(&self.description).kind == net::SocketKind::Tcp
                && matches!(
                    self.action,
                    Action::Connect {
                        finishing: false,
                        ..
                    }
                )
            {
                match Socket::state(&self.description).connection.get() {
                    Connection::Initial => {}
                    Connection::Connecting => {
                        let Some(ready) = self
                            .watch
                            .poll(Socket::state(&self.description))
                            .map_err(error)?
                        else {
                            return Ok(None);
                        };
                        if !self.action.ready(ready) {
                            return Err(EALREADY);
                        }
                        if let Action::Connect { finishing, .. } = &mut self.action {
                            *finishing = true;
                        }
                    }
                    Connection::Connected => return Err(EISCONN),
                }
            }
            native
                .start(self.action.request(Socket::state(&self.description).kind))
                .map_err(error)?;
            self.started = true;
            if Socket::state(&self.description).kind == net::SocketKind::Tcp
                && matches!(
                    self.action,
                    Action::Connect {
                        finishing: false,
                        ..
                    }
                )
            {
                Socket::state(&self.description)
                    .connection
                    .set(Connection::Connecting);
            }
        }
        let result = native.try_complete();
        if matches!(result, Err(net::Error::WouldBlock)) && native.has_pending() {
            return Ok(None);
        }
        if native.has_pending() {
            lease.fail();
            return Err(error(result.err().unwrap_or(net::Error::Io)));
        }
        self.started = false;
        self.lease = None;
        // Revisit other waiters now that this attempt has released the shared socket.
        self.progress = true;
        Ok(Some(result))
    }

    fn finish(
        &mut self,
        process: &mut Process<'a>,
        result: Result<net::Completion<'a>, net::Error>,
    ) -> Result<Option<usize>, Errno> {
        let connection = &Socket::state(&self.description).connection;
        if matches!(self.action, Action::GetOption { name: None, .. }) {
            return self.socket_error(process, result);
        }
        if Socket::state(&self.description).kind == net::SocketKind::Tcp
            && matches!(self.action, Action::Connect { .. })
        {
            connection.set(if result.is_ok() {
                Connection::Connected
            } else {
                Connection::Initial
            });
        }
        let completion = result.map_err(|reason| {
            if Socket::state(&self.description).kind == net::SocketKind::Tcp
                && matches!(self.action, Action::Receive(_))
                && reason == net::Error::ConnectionReset
            {
                Socket::state(&self.description).read_eof.set(true);
            }
            if matches!(self.action, Action::Send { .. }) && reason == net::Error::NotConnected {
                if Socket::state(&self.description).kind == net::SocketKind::Udp {
                    EDESTADDRREQ
                } else {
                    EPIPE
                }
            } else {
                error(reason)
            }
        })?;
        let value = match (&mut self.action, completion) {
            (
                Action::Accept { peer, flags },
                net::Completion::Accepted {
                    socket,
                    peer: endpoint,
                },
            ) => {
                if let Some(output) = peer {
                    output.endpoint(process, endpoint)?;
                }
                let state = Socket::state(&self.description);
                install(
                    process,
                    socket,
                    *flags,
                    true,
                    [state.receive_timeout.get(), state.send_timeout.get()],
                )?
            }
            (Action::Receive(receive), net::Completion::Data(data)) => {
                if data.is_empty() && Socket::state(&self.description).kind == net::SocketKind::Tcp
                {
                    if receive.buffers.total_length() != 0 {
                        Socket::state(&self.description).read_eof.set(true);
                    }
                    receive.empty(process)?;
                    return Ok(Some(0));
                }
                *Socket::state(&self.description).received.borrow_mut() =
                    Some(Received { data, offset: 0 });
                return Ok(None);
            }
            (Action::Send { .. }, net::Completion::Count(count)) => count,
            (Action::Endpoint { output, .. }, net::Completion::Endpoint(endpoint)) => {
                output.endpoint(process, endpoint)?
            }
            (Action::GetOption { output, .. }, net::Completion::Value(value)) => {
                output.value(process, value as usize)?
            }
            (
                Action::Bind(_)
                | Action::Listen(_)
                | Action::Connect { .. }
                | Action::Disconnect
                | Action::SetOption(_, _)
                | Action::Shutdown(_),
                net::Completion::Done,
            ) => 0,
            _ => return Err(EIO),
        };
        Ok(Some(value))
    }

    fn socket_error(
        &mut self,
        process: &Process<'_>,
        result: Result<net::Completion<'a>, net::Error>,
    ) -> Result<Option<usize>, Errno> {
        let connection = &Socket::state(&self.description).connection;
        if let Action::GetOption {
            name: None,
            output,
            checking: true,
        } = &self.action
        {
            if matches!(result, Ok(net::Completion::Endpoint(_))) {
                connection.set(Connection::Connected);
            }
            return output.value(process, 0).map(Some);
        }
        if let Action::GetOption {
            name: None, output, ..
        } = &self.action
        {
            let value = match result {
                Ok(net::Completion::Done) => {
                    if connection.get() == Connection::Connecting {
                        if let Action::GetOption { checking, .. } = &mut self.action {
                            *checking = true;
                        }
                        return Ok(None);
                    }
                    0
                }
                Ok(_) => return Err(EIO),
                Err(reason) => {
                    connection.set(Connection::Initial);
                    error(reason).number()
                }
            };
            return output.value(process, value).map(Some);
        }
        unreachable!()
    }
}
