// SPDX-License-Identifier: MPL-2.0

//! 消息通信、一次性回复、合并通知与多对象等待。

mod channel;
mod event;
mod reply;
mod timer;
pub(crate) mod wait;

pub(crate) use channel::MESSAGE_HANDLES;
pub use channel::{Channel, ChannelError, ChannelReference, Message, ReceivedMessage};
pub use event::{Event, EventError, EventReference};
pub(crate) use reply::Call;
pub use reply::{Reply, ReplyReference};
pub use timer::{Timer, TimerError, TimerReference};
pub(crate) use wait::WaitMode;
pub use wait::{
    DEAD, EXITED, PEER_CLOSED, READABLE, WRITABLE, WaitItem, WaitManyError, WaitManyResult,
};

pub(crate) fn next_timer_deadline() -> Option<u64> {
    timer::next_deadline()
}

pub(crate) fn timer_interrupt(now: u64) {
    timer::interrupt(now)
}
