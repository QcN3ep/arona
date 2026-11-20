// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Power session admission and action dispatch.

use super::{
    Error, Power,
    session::{CAPABILITIES, REBOOT, SHUTDOWN},
};
use crate::protocol::{INVALID_REQUEST, SUCCESS, error_status};
use crate::session::respond;
use runtime::{Channel, Handle, Reply, ipc::ReceivedMessage};

pub fn run(power: &mut Power<'_>, control: &Handle<Channel>, device: u32) -> Result<(), Error> {
    crate::session::serve(control, device, |request| receive(power, request))
}

fn receive(power: &mut Power<'_>, mut message: ReceivedMessage) -> Result<(), Error> {
    let Some(reply) = message.take_reply() else {
        return Ok(());
    };
    if !message.handles().is_empty() {
        respond(&reply, INVALID_REQUEST, &[])?;
        return Ok(());
    }
    match (message.tag, message.words()) {
        (CAPABILITIES, []) => {
            let capabilities = power.capabilities();
            respond(
                &reply,
                SUCCESS,
                &[capabilities.shutdown as u64, capabilities.reboot as u64],
            )?;
        }
        (SHUTDOWN, []) => action(reply, power.shutdown())?,
        (REBOOT, []) => action(reply, power.reboot())?,
        _ => {
            respond(&reply, INVALID_REQUEST, &[])?;
        }
    }
    Ok(())
}

fn action(reply: Handle<Reply>, result: Result<(), Error>) -> Result<(), Error> {
    match result {
        Ok(()) => respond(&reply, SUCCESS, &[])?,
        Err(error) => respond(&reply, error_status(error), &[])?,
    };
    Ok(())
}
