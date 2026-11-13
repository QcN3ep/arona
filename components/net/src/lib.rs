// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Native IP sockets served by a separate network Task.
//!
//! Each socket permits one outstanding operation. Timeouts preserve that
//! operation, and `Socket::complete` resumes it without repeating I/O.
//! Dropping a socket abandons its result and closes its service session.

#![no_std]
#![feature(allocator_api)]
#![forbid(unsafe_code)]

extern crate alloc;

mod client;
mod protocol;
mod types;

pub use client::{
    Client, Completion, Control, Data, PendingReadiness, PendingSubscription, Socket, Subscription,
};
pub use types::{
    AddressFamily, CancelOutcome, Endpoint, Error, IpAddr, Ipv4Addr, Ipv6Addr, OptionName,
    Readiness, Request, Shutdown, SocketKind,
};
