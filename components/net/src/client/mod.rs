// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

mod operation;
mod subscription;
pub use subscription::{PendingReadiness, PendingSubscription, Subscription};

use crate::{Error, Readiness, Request, SocketKind, protocol as p};
use runtime::{
    Channel, FrameAllocation, Handle, Rights, Task,
    ipc::{Message, ReceivedMessage, WaitMode},
};

/// Startup authority retained by init. Dropping its last client channel stops the service.
pub struct Control<'a>(Client<'a>);

impl<'a> Control<'a> {
    pub fn start(
        task: &'a Handle<Task>,
        channel: Handle<Channel>,
        entropy: devmgr::rng::Session<'_>,
        deadline: u64,
    ) -> Result<Self, Error> {
        let mut request = Message::new(p::START, &[])?;
        p::push(&mut request, entropy.into_channel(), p::CHANNEL)?;
        empty(call(&channel, &mut request, deadline)?)?;
        Ok(Self(Client::from_channel(task, channel)?))
    }

    pub fn connect(&self) -> Result<Client<'a>, Error> {
        Client::from_channel(self.0.task, self.0.channel.try_clone(p::OWNED_CHANNEL)?)
    }
}

/// Authority to create socket sessions, without access to devices or DMA.
pub struct Client<'a> {
    task: &'a Handle<Task>,
    channel: Handle<Channel>,
}

impl<'a> Client<'a> {
    pub fn from_channel(task: &'a Handle<Task>, channel: Handle<Channel>) -> Result<Self, Error> {
        if !channel.info()?.rights.contains(p::CHANNEL) {
            return Err(Error::InvalidRequest);
        }
        Ok(Self { task, channel })
    }
    pub fn channel(&self) -> &Handle<Channel> {
        &self.channel
    }

    pub fn into_channel(self) -> Handle<Channel> {
        self.channel
    }

    /// Begin socket creation. The returned socket retains the creation request.
    pub fn start_socket(&self, kind: SocketKind) -> Result<Socket<'a>, Error> {
        let (channel, server) = self.task.channel(p::OWNED_CHANNEL, p::OWNED_CHANNEL)?;
        let mut request = Message::new(p::ATTACH, &[])?;
        p::push(&mut request, server, p::OWNED_CHANNEL)?;
        self.channel.send(&mut request, WaitMode::Poll)?;
        let mut socket = Socket::new(self.task, channel, kind);
        socket.submit(
            p::CREATE,
            &[match kind {
                SocketKind::Tcp => 0,
                SocketKind::Udp => 1,
            }],
            None,
        )?;
        Ok(socket)
    }

    pub fn socket(&self, kind: SocketKind, mode: WaitMode) -> Result<Socket<'a>, Error> {
        let mut socket = self.start_socket(kind)?;
        match socket.complete(mode)? {
            Completion::Done => Ok(socket),
            _ => Err(Error::InvalidRequest),
        }
    }
}

/// One owned socket session. Operations require exclusive access to its request state.
pub struct Socket<'a> {
    task: &'a Handle<Task>,
    channel: Handle<Channel>,
    kind: SocketKind,
    next: u64,
    pending: Option<alloc::boxed::Box<operation::Pending<'a>>>,
}

/// A result that has been received and acknowledged exactly once by this client.
pub enum Completion<'a> {
    Done,
    Count(usize),
    Data(Data),
    Accepted {
        socket: Socket<'a>,
        peer: crate::Endpoint,
    },
    Endpoint(crate::Endpoint),
    Value(u64),
}

/// An owned received payload. Only READ rights arrive from the service.
pub struct Data {
    frames: Handle<FrameAllocation>,
    length: usize,
    pub peer: Option<crate::Endpoint>,
}

impl Data {
    pub fn len(&self) -> usize {
        self.length
    }
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }
    pub fn read(&self, offset: usize, output: &mut [u8]) -> Result<(), Error> {
        if offset
            .checked_add(output.len())
            .is_none_or(|end| end > self.length)
        {
            return Err(Error::InvalidRequest);
        }
        Ok(self.frames.read(offset, output)?)
    }
}

impl<'a> Socket<'a> {
    fn new(task: &'a Handle<Task>, channel: Handle<Channel>, kind: SocketKind) -> Self {
        Self {
            task,
            channel,
            kind,
            next: 1,
            pending: None,
        }
    }
    pub fn kind(&self) -> SocketKind {
        self.kind
    }
    pub fn channel(&self) -> &Handle<Channel> {
        &self.channel
    }
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn start(&mut self, request: Request<'_>) -> Result<(), Error> {
        if self.pending.is_some() {
            return Err(Error::Busy);
        }
        let encoded = operation::encode(request, self.kind)?;
        let frames = if let Some(bytes) = encoded.bytes {
            let mut frames = self.task.allocate(
                bytes.len().max(1).div_ceil(4096) as u64,
                Rights::READ_WRITE | Rights::TRANSFER,
            )?;
            frames.write(0, bytes)?;
            Some(frames)
        } else {
            None
        };
        self.submit(encoded.opcode, &encoded.arguments[..encoded.length], frames)
    }

    pub fn perform(
        &mut self,
        request: Request<'_>,
        mode: WaitMode,
    ) -> Result<Completion<'a>, Error> {
        self.start(request)?;
        self.complete(mode)
    }
}

pub fn readiness(words: &[u64]) -> Result<Readiness, Error> {
    let [sequence, flags] = *words else {
        return Err(Error::InvalidRequest);
    };
    if flags & !63 != 0 {
        return Err(Error::InvalidRequest);
    }
    Ok(Readiness {
        sequence,
        readable: flags & 1 != 0,
        writable: flags & 2 != 0,
        error: flags & 4 != 0,
        read_closed: flags & 8 != 0,
        write_closed: flags & 16 != 0,
        closed: flags & 32 != 0,
    })
}
pub fn empty(response: ReceivedMessage) -> Result<(), Error> {
    status(response.tag)?;
    if !response.words().is_empty() || !response.handles().is_empty() {
        return Err(Error::InvalidRequest);
    }
    Ok(())
}
pub fn call(
    channel: &Handle<Channel>,
    message: &mut Message,
    deadline: u64,
) -> Result<ReceivedMessage, Error> {
    channel
        .call(message, Some(deadline))
        .map_err(|error| match error {
            runtime::ipc::CallError::Submitted(error)
            | runtime::ipc::CallError::NotSubmitted(error) => error.into(),
        })
}

pub fn status(value: u64) -> Result<(), Error> {
    Err(match value {
        0 => return Ok(()),
        1 => Error::WouldBlock,
        2 => Error::InProgress,
        3 => Error::Busy,
        4 => Error::Canceled,
        5 => Error::InvalidRequest,
        6 => Error::NoMemory,
        7 => Error::Unavailable,
        8 => Error::Closed,
        9 => Error::AddressInUse,
        10 => Error::ConnectionRefused,
        11 => Error::ConnectionReset,
        12 => Error::NotConnected,
        13 => Error::BrokenPipe,
        14 => Error::TimedOut,
        15 => Error::Unsupported,
        16 => Error::Io,
        _ => Error::InvalidRequest,
    })
}
