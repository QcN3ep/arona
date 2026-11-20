// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Time queries, time writes, and close handling for RTC sessions.

use super::{
    Timestamp,
    session::{NOW, SET},
};
use crate::Error;
use crate::protocol::{INVALID_REQUEST, SUCCESS, error_status};
use crate::session::respond;
use runtime::{Channel, Handle, ipc::ReceivedMessage};

pub fn run(rtc: &mut super::Rtc<'_>, control: &Handle<Channel>, device: u32) -> Result<(), Error> {
    crate::session::serve(control, device, |request| receive(rtc, request))
}

fn receive(rtc: &mut super::Rtc<'_>, mut message: ReceivedMessage) -> Result<(), Error> {
    let Some(reply) = message.take_reply() else {
        return Ok(());
    };
    if !message.handles().is_empty() {
        respond(&reply, INVALID_REQUEST, &[])?;
        return Ok(());
    }
    if message.tag == NOW && message.words().is_empty() {
        return match rtc.now() {
            Ok(time) => respond(
                &reply,
                SUCCESS,
                &[time.seconds as u64, time.nanoseconds.into()],
            ),
            Err(error) => respond(&reply, error_status(error), &[]),
        }
        .map(|_| ());
    }
    if message.tag == SET {
        let status = match message.words() {
            [seconds, nanoseconds]
                if *nanoseconds < 1_000_000_000 && *seconds <= i64::MAX as u64 =>
            {
                match rtc.set(Timestamp {
                    seconds: *seconds as i64,
                    nanoseconds: *nanoseconds as u32,
                }) {
                    Ok(()) => SUCCESS,
                    Err(error) => error_status(error),
                }
            }
            _ => INVALID_REQUEST,
        };
        return respond(&reply, status, &[]).map(|_| ());
    }
    respond(&reply, INVALID_REQUEST, &[]).map(|_| ())
}
