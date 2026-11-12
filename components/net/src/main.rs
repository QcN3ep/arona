// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

mod protocol;
mod service;
use net::{Endpoint, Error, Readiness};

fn main() -> Result<(), Error> {
    service::run(runtime::process::bootstrap().ok_or(Error::InvalidRequest)?)
}
