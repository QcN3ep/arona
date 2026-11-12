// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::Socket;
use crate::{Error, Readiness, protocol as p};
use runtime::{
    Channel, Event, Handle, Rights,
    ipc::{self, Message, WaitItem, WaitMode},
};

/// Independent wake events and a channel that tracks the registration's lifetime.
pub struct Subscription {
    channel: Handle<Channel>,
    event: Handle<Event>,
}

impl Socket<'_> {
    pub fn readiness(&self, deadline: u64) -> Result<Readiness, Error> {
        snapshot(&self.channel, deadline)
    }
    pub fn subscribe(&self, deadline: u64) -> Result<(Subscription, Readiness), Error> {
        let (channel, server) = self
            .task
            .channel(p::CHANNEL, p::CHANNEL | Rights::TRANSFER)?;
        let mut request = Message::new(p::SUBSCRIBE, &[])?;
        p::push(&mut request, server, p::CHANNEL)?;
        let mut response = super::call(&self.channel, &mut request, deadline)?;
        super::status(response.tag)?;
        let initial = super::readiness(response.words())?;
        let event = p::take(&mut response, Rights::WAIT)?;
        if !response.handles().is_empty() {
            return Err(Error::InvalidRequest);
        }
        Ok((Subscription { channel, event }, initial))
    }
}
impl Subscription {
    pub fn event(&self) -> &Handle<Event> {
        &self.event
    }
    pub fn channel(&self) -> &Handle<Channel> {
        &self.channel
    }
    pub fn snapshot(&self, deadline: u64) -> Result<Readiness, Error> {
        snapshot(&self.channel, deadline)
    }
    /// A service or socket-session exit wakes this wait through peer closure.
    pub fn wait(&self, observed: Readiness, mode: WaitMode) -> Result<Readiness, Error> {
        loop {
            let deadline = match mode {
                WaitMode::Until(deadline) => deadline,
                _ => u64::MAX,
            };
            let state = self.snapshot(deadline)?;
            if state != observed {
                return Ok(state);
            }
            let ready = ipc::wait_many(
                &[
                    WaitItem::new(&self.channel, ipc::PEER_CLOSED),
                    WaitItem::new(&self.event, 1),
                ],
                mode,
            )?;
            if ready.index == 0 {
                return Err(Error::Unavailable);
            }
        }
    }
}
fn snapshot(channel: &Handle<Channel>, deadline: u64) -> Result<Readiness, Error> {
    let response = super::call(channel, &mut Message::new(p::SNAPSHOT, &[])?, deadline)?;
    super::status(response.tag)?;
    if !response.handles().is_empty() {
        return Err(Error::InvalidRequest);
    }
    super::readiness(response.words())
}
