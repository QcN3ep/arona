// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Native IPv4 sockets served by a separate network Task.
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

pub use client::{Client, Completion, Control, Data, Socket, Subscription};
pub use types::{
    CancelOutcome, Endpoint, Error, OptionName, Readiness, Request, Shutdown, SocketKind,
};
