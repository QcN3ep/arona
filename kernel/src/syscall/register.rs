// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Bounded register snapshots and atomic batch updates.

use super::error::CallError;
use crate::{
    hardware,
    memory::MemoryError,
    task::{ObjectReference, Task},
    trap::port::ForwardError,
};
use abi::{
    Error,
    register::{Write, WriteRequest},
};
use alloc::vec::Vec;

struct Group {
    id: u64,
    bytes: Vec<u8>,
}

pub(super) fn apply(
    caller: &Task,
    args: [u64; 5],
    layout: impl Fn(u64) -> Result<usize, ForwardError>,
    apply: impl FnOnce(&[Write<'_>]) -> Result<(), ForwardError>,
) -> Result<(), CallError> {
    let count = usize::try_from(args[1]).map_err(|_| Error::InvalidArgument)?;
    if args[2..] != [0; 3]
        || count > hardware::execution().register_groups().len()
        || count == 0 && args[0] != 0
    {
        return Err(Error::InvalidArgument.into());
    }
    let mut requests = Vec::<WriteRequest>::new();
    requests
        .try_reserve_exact(count)
        .map_err(|_| MemoryError::NoMemory)?;
    if count != 0 {
        let input = caller.pin_user_memory(args[0] as usize, count * WriteRequest::SIZE, false)?;
        for index in 0..count {
            let mut bytes = [0; WriteRequest::SIZE];
            input.read(index * WriteRequest::SIZE, &mut bytes)?;
            let request = WriteRequest::decode(&bytes);
            if requests.iter().any(|other| other.group == request.group) {
                return Err(Error::InvalidArgument.into());
            }
            requests.push(request);
        }
    }
    let mut groups = Vec::<Group>::new();
    groups
        .try_reserve_exact(count)
        .map_err(|_| MemoryError::NoMemory)?;
    for request in requests {
        let layout = layout(request.group)?;
        if request.length != layout as u64 {
            return Err(Error::InvalidArgument.into());
        }
        let length = layout;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|_| MemoryError::NoMemory)?;
        bytes.resize(length, 0);
        caller
            .pin_user_memory(request.address as usize, length, false)?
            .read(0, &mut bytes)?;
        groups.push(Group {
            id: request.group,
            bytes,
        });
    }
    let mut writes = Vec::new();
    writes
        .try_reserve_exact(groups.len())
        .map_err(|_| MemoryError::NoMemory)?;
    writes.extend(groups.iter().map(|group| Write {
        group: group.id,
        bytes: &group.bytes,
    }));
    Ok(apply(&writes)?)
}

pub(super) fn info(
    caller: &Task,
    target: &ObjectReference,
    args: [u64; 5],
) -> Result<[u64; 2], CallError> {
    let group = u64::from_le_bytes(super::info::query(caller, args[1], args[2])?);
    let layout = match target {
        ObjectReference::Thread(thread) => thread.register_size(group)?,
        ObjectReference::TrapContinuation(continuation) => continuation.register_size(group)?,
        _ => return Err(Error::Unsupported.into()),
    };
    let length = layout;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| MemoryError::NoMemory)?;
    bytes.resize(length, 0);
    match target {
        ObjectReference::Thread(thread) => thread.read_registers(group, &mut bytes)?,
        ObjectReference::TrapContinuation(continuation) => {
            continuation.read_registers(group, &mut bytes)?
        }
        _ => unreachable!(),
    }
    super::info::output(caller, args[3], args[4], &bytes)
}
