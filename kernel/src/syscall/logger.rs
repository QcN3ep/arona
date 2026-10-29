// SPDX-License-Identifier: MPL-2.0

use crate::{
    logger::{KernelLog, ReadError},
    task::Task,
};
use abi::{Error, logger::MAX_RECORD_SIZE};

pub(super) fn read(caller: &Task, target: &KernelLog, args: [u64; 5]) -> Result<[u64; 2], Error> {
    let mut bytes = [0; MAX_RECORD_SIZE];
    let length = target
        .read(args[0], &mut bytes)
        .map_err(|error| match error {
            ReadError::Busy => Error::Busy,
            ReadError::BufferTooSmall => Error::BufferTooSmall,
        })?;
    if args[2] < length as u64 {
        return Err(Error::BufferTooSmall);
    }
    if length != 0 {
        caller
            .pin_user_memory(args[1] as usize, length, true)?
            .write(0, &bytes[..length])?;
    }
    Ok([length as u64, 0])
}
