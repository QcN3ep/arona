// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Status encoding and response validation shared by device sessions.

use crate::Error;
use runtime::{
    Channel, Handle, KernelError,
    ipc::{Message, ReceivedMessage},
};

pub(crate) const SUCCESS: u64 = 0;

macro_rules! statuses {
    ($($name:ident = $value:literal => $error:ident),* $(,)?) => {
        $(pub(crate) const $name: u64 = $value;)*

        pub(crate) fn error_status(error: Error) -> u64 {
            match error {
                $(Error::$error => $name,)*
                Error::Runtime(runtime::Error::Kernel(KernelError::Busy)) => BUSY,
                _ => UNAVAILABLE,
            }
        }

        pub(crate) fn response_result(response: ReceivedMessage) -> Result<ReceivedMessage, Error> {
            if !response.handles().is_empty() {
                return Err(runtime::Error::Protocol.into());
            }
            if response.tag == SUCCESS {
                return Ok(response);
            }
            if !response.words().is_empty() {
                return Err(runtime::Error::Protocol.into());
            }
            Err(match response.tag {
                $($name => Error::$error,)*
                _ => runtime::Error::Protocol.into(),
            })
        }
    };
}

statuses! {
    INVALID_REQUEST = 1 => InvalidRequest,
    BUSY = 2 => Busy,
    UNSUPPORTED = 3 => Unsupported,
    INVALID_CONFIGURATION = 4 => InvalidConfiguration,
    CLOCK_UNAVAILABLE = 5 => ClockUnavailable,
    UNAVAILABLE = 6 => Unavailable,
    DENIED = 7 => Denied,
    NOT_FOUND = 8 => NotFound,
    DEVICE_FAULT = 9 => DeviceFault,
    READ_ONLY = 10 => ReadOnly,
}

pub(crate) fn empty(response: ReceivedMessage) -> Result<(), Error> {
    if response.words().is_empty() {
        Ok(())
    } else {
        Err(runtime::Error::Protocol.into())
    }
}

pub(crate) fn call(
    channel: &Handle<Channel>,
    request: &mut Message,
    deadline: Option<u64>,
) -> Result<ReceivedMessage, Error> {
    response_result(channel.call(request, deadline).map_err(Error::Call)?)
}

pub(crate) fn finite_deadline(deadline: u64) -> Result<Option<u64>, Error> {
    if matches!(deadline, 0 | u64::MAX) {
        Err(Error::InvalidRequest)
    } else {
        Ok(Some(deadline))
    }
}

pub(crate) fn call_until(
    channel: &Handle<Channel>,
    request: &mut Message,
    deadline: u64,
) -> Result<ReceivedMessage, Error> {
    call(channel, request, finite_deadline(deadline)?)
}
