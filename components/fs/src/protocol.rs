// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

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
pub const METADATA: u64 = 15;
pub const SET_ATTRIBUTES: u64 = 16;
pub const HARD_LINK: u64 = 17;
pub const SYMLINK: u64 = 18;
pub const READ_LINK: u64 = 19;
pub const APPEND: u64 = 20;
pub const OPEN_DIRECTORY: u64 = 21;
pub const RELATIVE: u64 = 22;
pub const DIRECTORY_PATH: u64 = 23;
pub const STATFS: u64 = 24;
pub const TMPFS: u64 = 25;
pub const SYNC: u64 = 26;
pub const MOUNT_TMPFS: u64 = 27;
pub const CONNECT_MOUNT: u64 = 29;
pub const UNMOUNT_MEMORY: u64 = 30;
pub const MOUNT_HUGEPAGES: u64 = 31;
pub const HUGEPAGE_INFO: u64 = 32;
pub const MAPPING_INFO: u64 = 34;
pub const MAP_PAGE: u64 = 35;
pub const VOLUME_INFO: u64 = 36;

pub const SUCCESS: u64 = 0;
macro_rules! statuses {
    ($($name:ident = $value:literal => $error:ident),* $(,)?) => {
        $(pub const $name: u64 = $value;)*

        fn error_tag(error: Error) -> u64 {
            match error {
                $(Error::$error => $name,)*
                _ => UNAVAILABLE,
            }
        }

        pub(crate) fn tag_error(tag: u64) -> Error {
            match tag {
                $($name => Error::$error,)*
                _ => Error::InvalidData,
            }
        }
    };
}

statuses! {
    INVALID_REQUEST = 1 => InvalidRequest,
    NOT_FOUND = 2 => NotFound,
    IS_DIRECTORY = 3 => IsDirectory,
    UNAVAILABLE = 4 => Unavailable,
    BUSY = 5 => Busy,
    READ_ONLY = 6 => ReadOnly,
    NO_SPACE = 7 => NoSpace,
    ALREADY_EXISTS = 8 => AlreadyExists,
    NOT_EMPTY = 9 => NotEmpty,
    CROSS_DEVICE = 10 => CrossDevice,
    NOT_DIRECTORY = 11 => NotDirectory,
    IO = 12 => Io,
    FILE_TOO_LARGE = 13 => FileTooLarge,
    TOO_MANY_LINKS = 14 => TooManyLinks,
    NAME_TOO_LONG = 15 => NameTooLong,
    UNSUPPORTED = 16 => Unsupported,
}

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

/// Sends a request and returns the raw reply.
///
/// The caller validates the status, payload, and transferred handles.
pub(crate) fn call_with_reply(
    channel: &Handle<Channel>,
    message: &mut Message,
    deadline: u64,
) -> Result<ReceivedMessage, Error> {
    if matches!(deadline, 0 | u64::MAX) {
        return Err(Error::InvalidRequest);
    }
    channel.call(message, Some(deadline)).map_err(Error::Call)
}

pub fn call(
    channel: &Handle<Channel>,
    message: &mut Message,
    deadline: u64,
) -> Result<ReceivedMessage, Error> {
    response_result(call_with_reply(channel, message, deadline)?)
}

pub(crate) fn response_result(response: ReceivedMessage) -> Result<ReceivedMessage, Error> {
    let response = response_status(response)?;
    if !response.handles().is_empty() {
        return Err(Error::InvalidData);
    }
    Ok(response)
}

pub(crate) fn response_status(response: ReceivedMessage) -> Result<ReceivedMessage, Error> {
    if response.tag == SUCCESS {
        return Ok(response);
    }
    if !response.words().is_empty() || !response.handles().is_empty() {
        return Err(Error::InvalidData);
    }
    Err(tag_error(response.tag))
}

pub fn respond(reply: Handle<Reply>, result: Result<Message, Error>) -> Result<(), Error> {
    let mut message = match result {
        Ok(message) => message,
        Err(error) => Message::new(error_tag(error), &[])?,
    };
    match reply.send(&mut message) {
        Ok(()) | Err(runtime::Error::Kernel(runtime::KernelError::InvalidState)) => Ok(()),
        Err(error) => Err(error.into()),
    }
}
