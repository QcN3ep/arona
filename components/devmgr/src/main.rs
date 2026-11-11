// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

mod service;

use devmgr::Error;
use runtime::{
    DeviceSet,
    ipc::{Message, WaitMode},
    process,
};

fn main() -> Result<(), Error> {
    let bootstrap = process::bootstrap().ok_or(runtime::Error::Protocol)?;
    let mut startup = bootstrap.receive(WaitMode::Forever)?;
    let reply = startup.take_reply();
    let mut devices = None;
    while let Some(handle) = startup.take_handle(0) {
        if let Ok(catalog) = handle.downcast::<DeviceSet>()
            && devices.replace(catalog).is_some()
        {
            return Err(runtime::Error::Protocol.into());
        }
    }
    let devices = devices.ok_or(runtime::Error::Protocol)?;
    drop(startup);
    let control = reply.as_ref().map(|_| bootstrap);
    let manager = service::DeviceManager::new(devices)?;
    if let Some(reply) = reply {
        reply.send(&mut Message::new(0, &[])?)?;
    }
    manager.run(control.as_ref())
}
