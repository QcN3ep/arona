// SPDX-License-Identifier: MPL-2.0

#![no_std]
#![forbid(unsafe_code)]

use devmgr::{
    Error,
    device::DeviceDescription,
    uart::{self, Settings, Uart},
};
use runtime::{
    DeviceSet, Rights,
    ipc::{Message, WaitMode},
    process, thread,
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
    let mut cursor = 0;
    while let Some(id) = devices.next(cursor)? {
        cursor = id;
        let device = DeviceDescription::new(&devices, id)?;
        if !device.info().console {
            continue;
        }
        let configuration = uart::probe(&device)?.ok_or(Error::Unsupported)?;
        let group = device.info().group.ok_or(Error::InvalidConfiguration)?;
        let grant = devices.claim_group(
            group,
            Rights::MAP
                .union(Rights::DEVICE_CONTROL)
                .union(Rights::IRQ_ACK),
        )?;
        let event = process::task().event(Rights::SIGNAL.union(Rights::WAIT))?;
        let mut uart = loop {
            match Uart::new(configuration, &grant, &event, 1, Settings::default()) {
                Ok(uart) => break uart,
                Err(Error::Busy) => thread::yield_now()?,
                Err(error) => return Err(error),
            }
        };
        if let Some(reply) = reply {
            reply.send(&mut Message::new(0, &[])?)?;
        }
        return uart::service::run(&mut uart, &event, control.as_ref(), id);
    }
    Err(Error::Unsupported)
}
