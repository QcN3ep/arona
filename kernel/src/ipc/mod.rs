// SPDX-License-Identifier: MPL-2.0

//! Message communication, one-shot replies, coalesced notifications, and multi-object waiting.

mod call;
mod channel;
mod event;
mod reply;
mod timer;
pub(crate) mod wait;

pub use call::{Call, CallReference};
pub(crate) use channel::MESSAGE_HANDLES;
pub use channel::{
    Channel, ChannelError, ChannelReference, Message, ReceivedMessage, active_channels,
    queued_messages,
};
pub use event::{Event, EventError, EventReference, active_events};
pub use reply::{Reply, ReplyReference, active_replies};
pub use timer::{Timer, TimerError, TimerReference, active_timers};
pub(crate) use wait::WaitMode;
pub use wait::active_selections;
pub use wait::{
    DEAD, EXITED, PEER_CLOSED, READABLE, WRITABLE, WaitItem, WaitManyError, WaitManyResult,
};

pub(crate) fn next_timer_deadline() -> Option<u64> {
    timer::next_deadline()
}

pub(crate) fn timer_interrupt(now: u64) {
    timer::interrupt(now)
}
