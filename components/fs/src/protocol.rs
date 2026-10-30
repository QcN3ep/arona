// SPDX-License-Identifier: MPL-2.0

use runtime::{
    Channel, Handle, Reply, Rights,
    ipc::{Message, ReceivedMessage, Transfer},
};

use crate::Error;

pub const START: u64 = 0;
pub const OPEN: u64 = 1;
pub const READ: u64 = 2;
pub const SUCCESS: u64 = 0;
pub const INVALID_REQUEST: u64 = 1;
pub const NOT_FOUND: u64 = 2;
pub const IS_DIRECTORY: u64 = 3;
pub const UNAVAILABLE: u64 = 4;
pub const CHANNEL_RIGHTS: Rights = Rights::READ_WRITE.union(Rights::WAIT);

pub fn transfer<T: runtime::Object>(
    message: &mut Message,
    handle: Handle<T>,
    rights: Rights,
) -> Result<(), Error> {
    message
        .push(Transfer {
            handle: handle.into(),
            rights,
        })
        .map_err(|_| Error::InvalidRequest)
}

pub fn call(
    channel: &Handle<Channel>,
    message: &mut Message,
    deadline: u64,
) -> Result<ReceivedMessage, Error> {
    if matches!(deadline, 0 | u64::MAX) {
        return Err(Error::InvalidRequest);
    }
    let response = channel.call(message, Some(deadline)).map_err(Error::Call)?;
    if !response.handles().is_empty() {
        return Err(Error::InvalidData);
    }
    if response.tag == SUCCESS {
        return Ok(response);
    }
    if !response.words().is_empty() {
        return Err(Error::InvalidData);
    }
    Err(match response.tag {
        INVALID_REQUEST => Error::InvalidRequest,
        NOT_FOUND => Error::NotFound,
        IS_DIRECTORY => Error::IsDirectory,
        UNAVAILABLE => Error::Unavailable,
        _ => Error::InvalidData,
    })
}

pub fn respond(reply: Handle<Reply>, result: Result<Message, Error>) -> Result<(), Error> {
    let mut message = match result {
        Ok(message) => message,
        Err(error) => Message::new(
            match error {
                Error::InvalidRequest => INVALID_REQUEST,
                Error::NotFound => NOT_FOUND,
                Error::IsDirectory => IS_DIRECTORY,
                _ => UNAVAILABLE,
            },
            &[],
        )?,
    };
    match reply.send(&mut message) {
        Ok(()) | Err(runtime::Error::Kernel(runtime::KernelError::InvalidState)) => Ok(()),
        Err(error) => Err(error.into()),
    }
}
