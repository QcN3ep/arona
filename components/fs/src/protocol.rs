// SPDX-License-Identifier: MPL-2.0

use runtime::{
    Channel, Handle, Reply, Rights,
    ipc::{Message, ReceivedMessage, Transfer},
};

use crate::Error;

pub const START: u64 = 0;
pub const OPEN: u64 = 1;
pub const READ: u64 = 2;
pub const CONNECT: u64 = 3;
pub const MOUNT: u64 = 4;
pub const UNMOUNT: u64 = 5;
pub const CREATE: u64 = 6;
pub const MKDIR: u64 = 7;
pub const UNLINK: u64 = 8;
pub const RENAME: u64 = 9;
pub const LIST: u64 = 10;
pub const WRITE: u64 = 11;
pub const TRUNCATE: u64 = 12;
pub const FILE_INFO: u64 = 13;
pub const FSYNC: u64 = 14;

pub const SUCCESS: u64 = 0;
pub const INVALID_REQUEST: u64 = 1;
pub const NOT_FOUND: u64 = 2;
pub const IS_DIRECTORY: u64 = 3;
pub const UNAVAILABLE: u64 = 4;
pub const BUSY: u64 = 5;
pub const READ_ONLY: u64 = 6;
pub const NO_SPACE: u64 = 7;
pub const ALREADY_EXISTS: u64 = 8;
pub const NOT_EMPTY: u64 = 9;
pub const CROSS_DEVICE: u64 = 10;
pub const NOT_DIRECTORY: u64 = 11;
pub const IO: u64 = 12;

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
        BUSY => Error::Busy,
        IO => Error::Io,
        READ_ONLY => Error::ReadOnly,
        NO_SPACE => Error::NoSpace,
        ALREADY_EXISTS => Error::AlreadyExists,
        NOT_EMPTY => Error::NotEmpty,
        CROSS_DEVICE => Error::CrossDevice,
        NOT_DIRECTORY => Error::NotDirectory,
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
                Error::Busy => BUSY,
                Error::Io => IO,
                Error::ReadOnly => READ_ONLY,
                Error::NoSpace => NO_SPACE,
                Error::AlreadyExists => ALREADY_EXISTS,
                Error::NotEmpty => NOT_EMPTY,
                Error::CrossDevice => CROSS_DEVICE,
                Error::NotDirectory => NOT_DIRECTORY,
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
