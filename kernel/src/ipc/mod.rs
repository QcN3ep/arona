// SPDX-License-Identifier: MPL-2.0

//! 消息通信与端点生命周期。

mod channel;

pub use channel::{Channel, ChannelError, ChannelReference, Message, ReceivedMessage};
pub(crate) use channel::{MESSAGE_HANDLES, WaitMode as ChannelWait};
