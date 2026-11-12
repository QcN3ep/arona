// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::{
    operation::{Command, Output},
    respond,
};
use crate::{Error, Readiness, protocol as p};
use alloc::vec::Vec;
use runtime::{
    Channel, Event, Handle, Reply, Rights,
    ipc::{self, Message, ReceivedMessage, WaitItem, WaitMode},
    process,
};

pub(super) struct Session {
    pub channel: Handle<Channel>,
    pub kind: Option<net::SocketKind>,
    pub socket: Option<lwip::Socket>,
    observer: Option<lwip::Subscription>,
    work: Option<Work>,
    retired: u64,
    watchers: Vec<Watcher>,
}
struct Watcher {
    channel: Handle<Channel>,
    event: Handle<Event>,
    seen: Readiness,
}
struct Work {
    id: u64,
    stage: Stage,
    reply: Option<Handle<Reply>>,
    defer: bool,
}
enum Stage {
    Queued(Command),
    Shutdown(lwip::PendingShutdown),
    Close(lwip::PendingClose),
    Done(Result<Output, Error>),
}

impl Session {
    pub(super) fn new(channel: Handle<Channel>) -> Self {
        Self {
            channel,
            kind: None,
            socket: None,
            observer: None,
            work: None,
            retired: 0,
            watchers: Vec::new(),
        }
    }
    pub(super) fn install(&mut self, socket: lwip::Socket, observer: lwip::Subscription) {
        self.kind = Some(match socket.kind() {
            lwip::SocketKind::Tcp => net::SocketKind::Tcp,
            lwip::SocketKind::Udp => net::SocketKind::Udp,
        });
        self.socket = Some(socket);
        self.observer = Some(observer);
    }
    pub(super) fn queued(&self) -> bool {
        self.work
            .as_ref()
            .is_some_and(|work| matches!(work.stage, Stage::Queued(_)))
    }
    fn snapshot(&self) -> Readiness {
        self.observer
            .as_ref()
            .map_or_else(Readiness::default, |observer| {
                let state = observer.snapshot();
                Readiness {
                    sequence: state.sequence,
                    readable: state.readable,
                    writable: state.writable,
                    error: state.error,
                    read_closed: state.read_closed,
                    write_closed: state.write_closed,
                    closed: state.closed,
                }
            })
    }
    pub(super) fn poll(&mut self) -> Result<bool, Error> {
        match self.channel.receive(WaitMode::Poll) {
            Ok(mut request) => {
                if let Some(reply) = request.take_reply() {
                    let mut reply = Some(reply);
                    if let Err(error) = self.request(&mut request, &mut reply)
                        && let Some(reply) = reply
                    {
                        let _ = respond(&reply, super::code(error), &[]);
                    }
                }
            }
            Err(runtime::Error::Kernel(runtime::KernelError::EndOfStream)) => return Ok(false),
            Err(runtime::Error::Kernel(runtime::KernelError::WouldBlock)) => {}
            Err(error) => return Err(error.into()),
        }
        let state = self.snapshot();
        let mut index = 0;
        while index < self.watchers.len() {
            let watcher = &mut self.watchers[index];
            match watcher.channel.receive(WaitMode::Poll) {
                Ok(mut request) => {
                    if let Some(reply) = request.take_reply() {
                        let valid = request.tag == p::SNAPSHOT
                            && request.words().is_empty()
                            && request.handles().is_empty();
                        let words = [state.sequence, super::flags(state)];
                        respond(
                            &reply,
                            if valid {
                                0
                            } else {
                                super::code(Error::InvalidRequest)
                            },
                            if valid { &words } else { &[] },
                        )?;
                    }
                }
                Err(runtime::Error::Kernel(runtime::KernelError::EndOfStream)) => {
                    self.watchers.swap_remove(index);
                    continue;
                }
                Err(runtime::Error::Kernel(runtime::KernelError::WouldBlock)) => {}
                Err(error) => return Err(error.into()),
            }
            if watcher.seen != state {
                watcher.event.signal(1)?;
                watcher.seen = state;
            }
            index += 1;
        }
        Ok(true)
    }
    fn request(
        &mut self,
        request: &mut ReceivedMessage,
        pending_reply: &mut Option<Handle<Reply>>,
    ) -> Result<(), Error> {
        let reply = pending_reply.as_ref().unwrap();
        if request.tag == p::SUBMIT {
            return self.submit(request, reply);
        }
        if request.tag == p::SUBSCRIBE {
            return self.subscribe(request, reply);
        }
        if !request.handles().is_empty() {
            return Err(Error::InvalidRequest);
        }
        if request.tag == p::SNAPSHOT && request.words().is_empty() {
            let state = self.snapshot();
            return respond(reply, 0, &[state.sequence, super::flags(state)]);
        }
        let [id] = *request.words() else {
            return Err(Error::InvalidRequest);
        };
        if request.tag == p::ACK && id != 0 && id <= self.retired {
            return respond(reply, 0, &[]);
        }
        let work = self
            .work
            .as_mut()
            .filter(|work| work.id == id)
            .ok_or(Error::InvalidRequest)?;
        match request.tag {
            p::RESULT => {
                if let Stage::Done(result) = &mut work.stage {
                    Output::respond(result, reply)
                } else {
                    work.reply = pending_reply.take();
                    Ok(())
                }
            }
            p::CANCEL => {
                let status = match work.stage {
                    Stage::Queued(_) => {
                        work.stage = Stage::Done(Err(Error::Canceled));
                        0
                    }
                    Stage::Done(_) => 1,
                    Stage::Shutdown(_) | Stage::Close(_) => 2,
                };
                respond(reply, 0, &[status])
            }
            p::ACK if matches!(work.stage, Stage::Done(_)) => {
                self.retired = id;
                self.work = None;
                respond(reply, 0, &[])
            }
            p::ACK => Err(Error::Busy),
            _ => Err(Error::InvalidRequest),
        }
    }
    fn submit(
        &mut self,
        request: &mut ReceivedMessage,
        reply: &Handle<Reply>,
    ) -> Result<(), Error> {
        let Some(&id) = request.words().first() else {
            return Err(Error::InvalidRequest);
        };
        if id == 0 || id <= self.retired {
            return Err(Error::InvalidRequest);
        }
        if let Some(work) = &self.work {
            return if work.id == id {
                respond(reply, 0, &[])
            } else {
                Err(Error::Busy)
            };
        }
        let stage = match Command::decode(request, self.kind) {
            Ok(command) => Stage::Queued(command),
            Err(error) => Stage::Done(Err(error)),
        };
        self.work = Some(Work {
            id,
            stage,
            reply: None,
            defer: true,
        });
        respond(reply, 0, &[])
    }
    fn subscribe(
        &mut self,
        request: &mut ReceivedMessage,
        reply: &Handle<Reply>,
    ) -> Result<(), Error> {
        if !request.words().is_empty() || request.handles().len() != 1 {
            return Err(Error::InvalidRequest);
        }
        let channel = p::take(request, p::CHANNEL)?;
        self.watchers.try_reserve(1).map_err(|_| Error::NoMemory)?;
        let event = process::task()
            .event(Rights::WAIT | Rights::SIGNAL | Rights::DUPLICATE | Rights::TRANSFER)?;
        let state = self.snapshot();
        let mut response = Message::new(0, &[state.sequence, super::flags(state)])?;
        p::push(
            &mut response,
            event.try_clone(Rights::WAIT | Rights::TRANSFER)?,
            Rights::WAIT,
        )?;
        reply.send(&mut response)?;
        self.watchers.push(Watcher {
            channel,
            event,
            seen: state,
        });
        Ok(())
    }
    pub(super) fn progress(
        &mut self,
        stack: &lwip::Stack,
        reserve: Result<(), Error>,
    ) -> Result<Option<Session>, Error> {
        let Some(mut work) = self.work.take() else {
            return Ok(None);
        };
        let mut child = None;
        work.stage = match work.stage {
            Stage::Queued(command) if work.defer => {
                work.defer = false;
                Stage::Queued(command)
            }
            Stage::Queued(Command::Shutdown(direction)) => match self.socket.take() {
                Some(socket) => Stage::Shutdown(socket.shutdown(direction)),
                None => Stage::Done(Err(Error::Closed)),
            },
            Stage::Queued(Command::Close) => match self.socket.take() {
                Some(socket) => Stage::Close(socket.close()),
                None => Stage::Done(Err(Error::Closed)),
            },
            Stage::Queued(command) => Stage::Done(command.execute(self, stack, reserve).map(
                |(output, accepted)| {
                    child = accepted;
                    output
                },
            )),
            Stage::Shutdown(mut operation) => match operation.try_complete() {
                Some((socket, result)) => {
                    self.socket = Some(socket);
                    Stage::Done(result.map(|()| Output::done()).map_err(super::error))
                }
                None => Stage::Shutdown(operation),
            },
            Stage::Close(operation) if operation.is_complete() => Stage::Done(Ok(Output::done())),
            stage => stage,
        };
        if let Stage::Done(result) = &mut work.stage
            && let Some(reply) = work.reply.take()
        {
            let _ = Output::respond(result, &reply);
        }
        self.work = Some(work);
        Ok(child)
    }
    pub(super) fn wait_count(&self) -> usize {
        3 + self.watchers.len()
    }
    pub(super) fn wait_items<'a>(&'a self, waits: &mut Vec<WaitItem<'a>>) {
        waits.push(WaitItem::new(
            &self.channel,
            ipc::READABLE | ipc::PEER_CLOSED,
        ));
        if let Some(observer) = &self.observer {
            waits.push(WaitItem::new(observer.event(), 1));
        }
        if let Some(work) = &self.work {
            match &work.stage {
                Stage::Shutdown(operation) => {
                    if let Some(event) = operation.event() {
                        waits.push(WaitItem::new(event, 1));
                    }
                }
                Stage::Close(operation) => waits.push(WaitItem::new(operation.event(), 1)),
                _ => {}
            }
        }
        for watcher in &self.watchers {
            waits.push(WaitItem::new(
                &watcher.channel,
                ipc::READABLE | ipc::PEER_CLOSED,
            ));
        }
    }
}
