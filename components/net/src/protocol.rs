// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use crate::{Endpoint, Error};
use runtime::{
    AnyHandle, Handle, Object, Rights,
    ipc::{Message, ReceivedMessage, Transfer},
};

pub const CHANNEL: Rights = Rights::READ_WRITE.union(Rights::WAIT);
pub const OWNED_CHANNEL: Rights = CHANNEL.union(Rights::DUPLICATE).union(Rights::TRANSFER);
pub const START: u64 = 0;
pub const ATTACH: u64 = 1;
pub const SUBMIT: u64 = 2;
pub const RESULT: u64 = 3;
pub const CANCEL: u64 = 4;
pub const ACK: u64 = 5;
pub const SNAPSHOT: u64 = 6;
pub const SUBSCRIBE: u64 = 7;
pub const MAX_STREAM: usize = 65536;
pub const MAX_DATAGRAM: usize = 65507;

pub const CREATE: u64 = 0;
pub const BIND: u64 = 1;
pub const LISTEN: u64 = 2;
pub const CONNECT: u64 = 3;
pub const ACCEPT: u64 = 4;
pub const SEND: u64 = 5;
pub const SEND_TO: u64 = 6;
pub const RECEIVE: u64 = 7;
pub const RECEIVE_DATAGRAM: u64 = 8;
pub const LOCAL: u64 = 9;
pub const PEER: u64 = 10;
pub const TAKE_ERROR: u64 = 11;
pub const GET_OPTION: u64 = 12;
pub const SET_OPTION: u64 = 13;
pub const SHUTDOWN: u64 = 14;
pub const CLOSE: u64 = 15;
pub const DISCONNECT: u64 = 16;

pub const DONE: u64 = 0;
pub const COUNT: u64 = 1;
pub const DATA: u64 = 2;
pub const ACCEPTED: u64 = 3;
pub const ENDPOINT: u64 = 4;
pub const VALUE: u64 = 5;

pub fn endpoint(value: Endpoint) -> u64 {
    u64::from(u32::from_be_bytes(value.address)) | (u64::from(value.port) << 32)
}
pub fn decode_endpoint(value: u64) -> Result<Endpoint, Error> {
    if value >> 48 != 0 {
        return Err(Error::InvalidRequest);
    }
    Ok(Endpoint {
        address: (value as u32).to_be_bytes(),
        port: (value >> 32) as u16,
    })
}
pub fn push<T: Object>(
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
pub fn take<T: Object>(message: &mut ReceivedMessage, rights: Rights) -> Result<Handle<T>, Error> {
    let handle = message.take_handle(0).ok_or(Error::InvalidRequest)?;
    checked(handle, rights)
}
fn checked<T: Object>(handle: AnyHandle, rights: Rights) -> Result<Handle<T>, Error> {
    let handle = handle.downcast::<T>().map_err(|_| Error::InvalidRequest)?;
    if !handle.info()?.rights.contains(rights) {
        return Err(Error::InvalidRequest);
    }
    Ok(handle)
}
