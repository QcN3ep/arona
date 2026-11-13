// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::{Completion, Data, Socket};
use crate::{CancelOutcome, Error, OptionName, Request, Shutdown, SocketKind, protocol as p};
use runtime::{
    Call, FrameAllocation, Handle, Rights,
    ipc::{self, Message, ReceivedMessage, WaitItem, WaitMode},
};

pub(super) struct Pending<'a> {
    id: u64,
    phase: Phase,
    call: Option<Handle<Call>>,
    result: Option<Result<Completion<'a>, Error>>,
}
#[derive(Clone, Copy)]
enum Phase {
    Submit,
    Retrieve,
    Acknowledge,
}

impl<'a> Socket<'a> {
    pub(super) fn submit(
        &mut self,
        opcode: u64,
        arguments: &[u64],
        frames: Option<Handle<FrameAllocation>>,
    ) -> Result<(), Error> {
        if self.pending.is_some() {
            return Err(Error::Busy);
        }
        let id = self.next;
        let next = id.checked_add(1).ok_or(Error::InvalidRequest)?;
        let mut words = [id, opcode, 0, 0, 0];
        words[2..2 + arguments.len()].copy_from_slice(arguments);
        let mut message = Message::new(p::SUBMIT, &words[..2 + arguments.len()])?;
        if let Some(frames) = frames {
            p::push(&mut message, frames, Rights::READ)?;
        }
        let mut pending = alloc::boxed::Box::try_new(Pending {
            id,
            phase: Phase::Submit,
            call: None,
            result: None,
        })
        .map_err(|_| Error::NoMemory)?;
        pending.call = Some(self.channel.start_call(&mut message)?);
        self.pending = Some(pending);
        self.next = next;
        Ok(())
    }

    /// The current reply wait, including result acknowledgement. A lost wait
    /// leaves the operation identity in this socket until completion or drop.
    pub fn call(&self) -> Option<&Handle<Call>> {
        self.pending.as_ref()?.call.as_ref()
    }

    /// Drop only the reply wait. The next completion attempt retrieves the
    /// retained result, or repeats an acknowledgement already in progress.
    pub fn forget_reply(&mut self) {
        if let Some(pending) = &mut self.pending {
            pending.call = None;
            if matches!(pending.phase, Phase::Submit) {
                pending.phase = Phase::Retrieve;
            }
        }
    }

    pub fn cancel(&mut self, deadline: u64) -> Result<CancelOutcome, Error> {
        let id = self.pending.as_ref().ok_or(Error::InvalidRequest)?.id;
        let response = super::call(
            &self.channel,
            &mut Message::new(p::CANCEL, &[id])?,
            deadline,
        )?;
        super::status(response.tag)?;
        if !response.handles().is_empty() {
            return Err(Error::InvalidRequest);
        }
        match response.words() {
            [0] => Ok(CancelOutcome::Canceled),
            [1] => Ok(CancelOutcome::Committed),
            [2] => Ok(CancelOutcome::Pending),
            _ => Err(Error::InvalidRequest),
        }
    }

    pub fn try_complete(&mut self) -> Result<Completion<'a>, Error> {
        loop {
            let pending = self.pending.as_mut().ok_or(Error::InvalidRequest)?;
            if pending.call.is_none() {
                let tag = if matches!(pending.phase, Phase::Acknowledge) {
                    p::ACK
                } else {
                    p::RESULT
                };
                pending.call = Some(
                    self.channel
                        .start_call(&mut Message::new(tag, &[pending.id])?)?,
                );
            }
            let response = match pending.call.as_ref().unwrap().try_receive() {
                Ok(response) => response,
                Err(
                    error @ runtime::Error::Kernel(
                        runtime::KernelError::EndOfStream | runtime::KernelError::InvalidState,
                    ),
                ) => {
                    pending.call = None;
                    if matches!(pending.phase, Phase::Submit) {
                        pending.phase = Phase::Retrieve;
                    }
                    return Err(error.into());
                }
                Err(error) => return Err(error.into()),
            };
            pending.call = None;
            match pending.phase {
                Phase::Submit => {
                    super::empty(response)?;
                    pending.phase = Phase::Retrieve;
                }
                Phase::Retrieve => {
                    super::status(response.tag)?;
                    pending.result = Some(decode(self.task, response));
                    pending.phase = Phase::Acknowledge;
                }
                Phase::Acknowledge => {
                    super::empty(response)?;
                    return self.pending.take().unwrap().result.unwrap();
                }
            }
        }
    }

    /// Complete the current request. A timeout keeps its identity and any
    /// delivered payload or accepted socket for the next call to this method.
    pub fn complete(&mut self, mode: WaitMode) -> Result<Completion<'a>, Error> {
        loop {
            match self.try_complete() {
                Err(Error::WouldBlock) => {
                    let call = self.call().ok_or(Error::WouldBlock)?;
                    ipc::wait_many(&[WaitItem::new(call, ipc::READABLE)], mode)?;
                }
                result => return result,
            }
        }
    }
}

fn decode(
    task: &Handle<runtime::Task>,
    mut response: ReceivedMessage,
) -> Result<Completion<'_>, Error> {
    super::status(response.tag)?;
    let (&status, words) = response
        .words()
        .split_first()
        .ok_or(Error::InvalidRequest)?;
    super::status(status)?;
    let mut fields = [0; 4];
    if words.len() > fields.len() {
        return Err(Error::InvalidRequest);
    }
    fields[..words.len()].copy_from_slice(words);
    let length = words.len();
    let result = match &fields[..length] {
        [p::DONE] => Completion::Done,
        [p::COUNT, count] if *count <= p::MAX_STREAM as u64 => Completion::Count(*count as usize),
        [p::VALUE, value] => Completion::Value(*value),
        [p::ENDPOINT, endpoint] => Completion::Endpoint(p::decode_endpoint(*endpoint)?),
        [p::DATA, length, has_peer @ 0..=1, peer] if *length <= p::MAX_STREAM as u64 => {
            let frames = p::take::<FrameAllocation>(&mut response, Rights::READ)?;
            if frames.frames()? * 4096 < *length {
                return Err(Error::InvalidRequest);
            }
            Completion::Data(Data {
                frames,
                length: *length as usize,
                peer: if *has_peer == 1 {
                    Some(p::decode_endpoint(*peer)?)
                } else {
                    None
                },
            })
        }
        [p::ACCEPTED, peer] => Completion::Accepted {
            socket: Socket::new(task, p::take(&mut response, p::CHANNEL)?, SocketKind::Tcp),
            peer: p::decode_endpoint(*peer)?,
        },
        _ => return Err(Error::InvalidRequest),
    };
    if !response.handles().is_empty() {
        return Err(Error::InvalidRequest);
    }
    Ok(result)
}

pub(super) struct Encoded<'a> {
    pub opcode: u64,
    pub arguments: [u64; 3],
    pub length: usize,
    pub bytes: Option<&'a [u8]>,
}
impl<'a> Encoded<'a> {
    fn new(opcode: u64, arguments: &[u64], bytes: Option<&'a [u8]>) -> Self {
        let mut fields = [0; 3];
        fields[..arguments.len()].copy_from_slice(arguments);
        Self {
            opcode,
            arguments: fields,
            length: arguments.len(),
            bytes,
        }
    }
}
pub(super) fn encode(request: Request<'_>, kind: SocketKind) -> Result<Encoded<'_>, Error> {
    let option = |option| match option {
        OptionName::ReuseAddress => 0,
        OptionName::NoDelay => 1,
    };
    let encoded = match request {
        Request::Bind(endpoint) => Encoded::new(p::BIND, &[p::endpoint(endpoint)], None),
        Request::Listen(backlog) => Encoded::new(p::LISTEN, &[backlog.into()], None),
        Request::Connect(endpoint) => Encoded::new(p::CONNECT, &[p::endpoint(endpoint)], None),
        Request::Disconnect if kind == SocketKind::Udp => Encoded::new(p::DISCONNECT, &[], None),
        Request::Disconnect => return Err(Error::InvalidRequest),
        Request::Accept => Encoded::new(p::ACCEPT, &[], None),
        Request::Send(bytes) => Encoded::new(p::SEND, &[0, bytes.len() as u64], Some(bytes)),
        Request::SendTo(bytes, endpoint) => Encoded::new(
            p::SEND_TO,
            &[0, bytes.len() as u64, p::endpoint(endpoint)],
            Some(bytes),
        ),
        Request::Receive(length) if kind == SocketKind::Tcp && length <= p::MAX_STREAM => {
            Encoded::new(p::RECEIVE, &[length as u64], None)
        }
        Request::Receive(_) => return Err(Error::InvalidRequest),
        Request::ReceiveDatagram if kind == SocketKind::Udp => {
            Encoded::new(p::RECEIVE_DATAGRAM, &[], None)
        }
        Request::ReceiveDatagram => return Err(Error::InvalidRequest),
        Request::LocalEndpoint => Encoded::new(p::LOCAL, &[], None),
        Request::PeerEndpoint => Encoded::new(p::PEER, &[], None),
        Request::TakeError => Encoded::new(p::TAKE_ERROR, &[], None),
        Request::GetOption(name) => Encoded::new(p::GET_OPTION, &[option(name)], None),
        Request::SetOption(name, value) => {
            Encoded::new(p::SET_OPTION, &[option(name), u64::from(value)], None)
        }
        Request::Shutdown(direction) => Encoded::new(
            p::SHUTDOWN,
            &[match direction {
                Shutdown::Read => 0,
                Shutdown::Write => 1,
                Shutdown::Both => 2,
            }],
            None,
        ),
        Request::Close => Encoded::new(p::CLOSE, &[], None),
    };
    if encoded.bytes.is_some_and(|bytes| {
        bytes.len()
            > match kind {
                SocketKind::Tcp => p::MAX_STREAM,
                SocketKind::Udp => p::MAX_DATAGRAM,
            }
    }) {
        return Err(Error::InvalidRequest);
    }
    Ok(encoded)
}
