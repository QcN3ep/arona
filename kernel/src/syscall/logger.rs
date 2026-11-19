// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::error::CallError;
use crate::{
    logger::{KernelLog, ReadError},
    task::Task,
};
use abi::{Error, logger::MAX_RECORD_SIZE};

pub(super) fn read(
    caller: &Task,
    target: &KernelLog,
    args: [u64; 5],
) -> Result<[u64; 2], CallError> {
    if args[3..] != [0; 2] {
        return Err(Error::InvalidArgument.into());
    }
    let mut bytes = [0; MAX_RECORD_SIZE];
    let length = target
        .read(args[0], &mut bytes)
        .map_err(|error| match error {
            ReadError::Busy => CallError::from(Error::Busy),
            ReadError::BufferTooSmall(required) => CallError::buffer_too_small(required),
        })?;
    if args[2] < length as u64 {
        return Err(CallError::buffer_too_small(length));
    }
    if length != 0 {
        caller
            .pin_user_memory(args[1] as usize, length, true)?
            .write(0, &bytes[..length])?;
    }
    Ok([length as u64, 0])
}
