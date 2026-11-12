// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::{error, respond, session::Session};
use crate::{Error, protocol as p};
use alloc::vec::Vec;
use runtime::{
    Channel, FrameAllocation, Handle, Reply, Rights,
    ipc::{Message, ReceivedMessage},
    process,
};

pub(super) enum Command {
    Create(lwip::SocketKind),
    Bind(lwip::Endpoint),
    Listen(u8),
    Connect(lwip::Endpoint),
    Accept,
    Send(Vec<u8>, Option<lwip::Endpoint>),
    Receive(usize, bool),
    Endpoint(bool),
    TakeError,
    GetOption(lwip::OptionName),
    SetOption(lwip::OptionName, bool),
    Shutdown(lwip::Shutdown),
    Close,
}

pub(super) struct Output {
    words: [u64; 4],
    length: usize,
    channel: Option<Handle<Channel>>,
    data: Option<(Handle<FrameAllocation>, Vec<u8>)>,
}
impl Output {
    fn new(words: &[u64]) -> Self {
        let mut output = Self {
            words: [0; 4],
            length: words.len(),
            channel: None,
            data: None,
        };
        output.words[..words.len()].copy_from_slice(words);
        output
    }
    pub(super) fn done() -> Self {
        Self::new(&[p::DONE])
    }
    pub(super) fn respond(
        result: &mut Result<Self, Error>,
        reply: &Handle<Reply>,
    ) -> Result<(), Error> {
        let output = match result {
            Ok(output) => output,
            Err(error) => return respond(reply, 0, &[super::code(*error)]),
        };
        match output.message() {
            Ok(mut message) => match reply.send(&mut message) {
                Ok(()) | Err(runtime::Error::Kernel(runtime::KernelError::InvalidState)) => Ok(()),
                Err(error) => respond(reply, super::code(error.into()), &[]),
            },
            Err(error) => respond(reply, super::code(error), &[]),
        }
    }
    fn message(&mut self) -> Result<Message, Error> {
        let mut words = [0; 5];
        words[1..1 + self.length].copy_from_slice(&self.words[..self.length]);
        let mut message = Message::new(0, &words[..1 + self.length])?;
        if let Some(channel) = &self.channel {
            p::push(
                &mut message,
                channel.try_clone(p::OWNED_CHANNEL)?,
                p::OWNED_CHANNEL,
            )?;
        }
        if let Some((frames, bytes)) = &mut self.data {
            frames.write(0, bytes)?;
            p::push(
                &mut message,
                frames.try_clone(Rights::READ | Rights::TRANSFER)?,
                Rights::READ,
            )?;
        }
        Ok(message)
    }
}

impl Command {
    pub(super) fn decode(
        request: &mut ReceivedMessage,
        kind: Option<net::SocketKind>,
    ) -> Result<Self, Error> {
        let option = |raw| match raw {
            0 => Ok(lwip::OptionName::ReuseAddress),
            1 => Ok(lwip::OptionName::NoDelay),
            _ => Err(Error::Unsupported),
        };
        let command = match *request.words() {
            [_, p::CREATE, value @ 0..=1] if kind.is_none() => Self::Create(if value == 0 {
                lwip::SocketKind::Tcp
            } else {
                lwip::SocketKind::Udp
            }),
            [_, p::BIND, address] => Self::Bind(endpoint(address)?),
            [_, p::LISTEN, backlog] if backlog <= u8::MAX as u64 => Self::Listen(backlog as u8),
            [_, p::CONNECT, address] => Self::Connect(endpoint(address)?),
            [_, p::ACCEPT] => Self::Accept,
            [_, p::SEND | p::SEND_TO, ..] => Self::send(request, kind)?,
            [_, p::RECEIVE, length]
                if kind == Some(net::SocketKind::Tcp) && length <= p::MAX_STREAM as u64 =>
            {
                Self::Receive(length as usize, false)
            }
            [_, p::RECEIVE_DATAGRAM] if kind == Some(net::SocketKind::Udp) => {
                Self::Receive(p::MAX_DATAGRAM, true)
            }
            [_, p::LOCAL] => Self::Endpoint(false),
            [_, p::PEER] => Self::Endpoint(true),
            [_, p::TAKE_ERROR] => Self::TakeError,
            [_, p::GET_OPTION, name] => Self::GetOption(option(name)?),
            [_, p::SET_OPTION, name, value @ 0..=1] => Self::SetOption(option(name)?, value != 0),
            [_, p::SHUTDOWN, direction @ 0..=2] => Self::Shutdown(match direction {
                0 => lwip::Shutdown::Read,
                1 => lwip::Shutdown::Write,
                _ => lwip::Shutdown::Both,
            }),
            [_, p::CLOSE] => Self::Close,
            _ => return Err(Error::InvalidRequest),
        };
        if !request.handles().is_empty() {
            return Err(Error::InvalidRequest);
        }
        Ok(command)
    }

    fn send(request: &mut ReceivedMessage, kind: Option<net::SocketKind>) -> Result<Self, Error> {
        let [_, op, offset, length, ..] = *request.words() else {
            return Err(Error::InvalidRequest);
        };
        let peer = match request.words() {
            [_, p::SEND_TO, _, _, raw] => Some(endpoint(*raw)?),
            [_, p::SEND, _, _] => None,
            _ => return Err(Error::InvalidRequest),
        };
        let limit = match kind {
            Some(net::SocketKind::Tcp) => p::MAX_STREAM,
            Some(net::SocketKind::Udp) => p::MAX_DATAGRAM,
            None => return Err(Error::Closed),
        };
        if length > limit as u64
            || request.handles().len() != 1
            || (op == p::SEND_TO && kind != Some(net::SocketKind::Udp))
        {
            return Err(Error::InvalidRequest);
        }
        let frames = p::take::<FrameAllocation>(request, Rights::READ)?;
        if offset.checked_add(length).is_none_or(|end| {
            frames
                .frames()
                .ok()
                .and_then(|count| count.checked_mul(4096))
                .is_none_or(|size| end > size)
        }) {
            return Err(Error::InvalidRequest);
        }
        let mut bytes = buffer(length as usize)?;
        frames.read(
            usize::try_from(offset).map_err(|_| Error::InvalidRequest)?,
            &mut bytes,
        )?;
        Ok(Self::Send(bytes, peer))
    }

    pub(super) fn execute(
        self,
        session: &mut Session,
        stack: &lwip::Stack,
        reserve: Result<(), Error>,
    ) -> Result<(Output, Option<Session>), Error> {
        if let Self::Create(kind) = self {
            let (socket, observer) = stack.socket_with_subscription(kind).map_err(error)?;
            session.install(socket, observer);
            return Ok((Output::done(), None));
        }
        let socket = session.socket.as_mut().ok_or(Error::Closed)?;
        let output = match self {
            Self::Bind(endpoint) => {
                socket.bind(endpoint).map_err(error)?;
                Output::done()
            }
            Self::Listen(backlog) => {
                socket.listen(backlog).map_err(error)?;
                Output::done()
            }
            Self::Connect(endpoint) => {
                socket.connect(endpoint).map_err(error)?;
                Output::done()
            }
            Self::Send(bytes, peer) => {
                let length = match peer {
                    Some(peer) => socket.send_to(&bytes, peer),
                    None => socket.send(&bytes),
                }
                .map_err(error)?;
                Output::new(&[p::COUNT, length as u64])
            }
            Self::Receive(length, datagram) => receive(socket, length, datagram)?,
            Self::Accept => {
                reserve?;
                let (client, server) =
                    process::task().channel(p::OWNED_CHANNEL, p::OWNED_CHANNEL)?;
                let mut child = Session::new(server);
                let (accepted, observer, peer) =
                    socket.accept_with_subscription().map_err(error)?;
                child.install(accepted, observer);
                let mut output = Output::new(&[p::ACCEPTED, wire_endpoint(peer)]);
                output.channel = Some(client);
                return Ok((output, Some(child)));
            }
            Self::Endpoint(peer) => Output::new(&[
                p::ENDPOINT,
                wire_endpoint(
                    if peer {
                        socket.peer_endpoint()
                    } else {
                        socket.local_endpoint()
                    }
                    .map_err(error)?,
                ),
            ]),
            Self::TakeError => {
                socket.take_error().map_err(error)?;
                Output::done()
            }
            Self::GetOption(option) => {
                Output::new(&[p::VALUE, u64::from(socket.option(option).map_err(error)?)])
            }
            Self::SetOption(option, value) => {
                socket.set_option(option, value).map_err(error)?;
                Output::done()
            }
            Self::Create(_) | Self::Shutdown(_) | Self::Close => unreachable!(),
        };
        Ok((output, None))
    }
}
fn wire_endpoint(value: lwip::Endpoint) -> u64 {
    p::endpoint(net::Endpoint {
        address: value.address,
        port: value.port,
    })
}
fn buffer(length: usize) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| Error::NoMemory)?;
    bytes.resize(length, 0);
    Ok(bytes)
}

fn endpoint(raw: u64) -> Result<lwip::Endpoint, Error> {
    let value = p::decode_endpoint(raw)?;
    Ok(lwip::Endpoint {
        address: value.address,
        port: value.port,
    })
}

fn receive(socket: &mut lwip::Socket, length: usize, datagram: bool) -> Result<Output, Error> {
    let mut bytes = buffer(length)?;
    let frames = process::task().allocate(
        length.max(1).div_ceil(4096) as u64,
        Rights::READ_WRITE | Rights::DUPLICATE | Rights::TRANSFER,
    )?;
    let (length, peer) = if datagram {
        let (count, peer) = socket.receive_from(&mut bytes).map_err(error)?;
        (count, Some(peer))
    } else {
        (socket.receive(&mut bytes).map_err(error)?, None)
    };
    bytes.truncate(length);
    let mut output = Output::new(&[
        p::DATA,
        length as u64,
        u64::from(peer.is_some()),
        peer.map_or(0, wire_endpoint),
    ]);
    output.data = Some((frames, bytes));
    Ok(output)
}
