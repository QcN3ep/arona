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
    if requests.len() <= 2
        && requests
            .iter()
            .all(|request| matches!(request.group, abi::register::GENERAL | abi::register::FLOAT))
    {
        let mut storage = [[0; abi::register::ScalarFloat::LOONGARCH_SIZE]; 2];
        let mut writes = arrayvec::ArrayVec::<Write<'_>, 2>::new();
        for (request, bytes) in requests.iter().zip(&mut storage) {
            let length = layout(request.group)?;
            let bytes = bytes.get_mut(..length).ok_or(Error::Unsupported)?;
            read_group(caller, request, bytes)?;
            writes.push(Write {
                group: request.group,
                bytes,
            });
        }
        return Ok(apply(&writes)?);
    }
    let mut groups = Vec::<Group>::new();
    groups
        .try_reserve_exact(count)
        .map_err(|_| MemoryError::NoMemory)?;
    for request in requests {
        let layout = layout(request.group)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(layout)
            .map_err(|_| MemoryError::NoMemory)?;
        bytes.resize(layout, 0);
        read_group(caller, &request, &mut bytes)?;
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

fn read_group(caller: &Task, request: &WriteRequest, bytes: &mut [u8]) -> Result<(), CallError> {
    if request.length != bytes.len() as u64 {
        return Err(Error::InvalidArgument.into());
    }
    Ok(caller
        .pin_user_memory(request.address as usize, bytes.len(), false)?
        .read(0, bytes)?)
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
    with_bytes(layout, |bytes| {
        match target {
            ObjectReference::Thread(thread) => thread.read_registers(group, bytes)?,
            ObjectReference::TrapContinuation(continuation) => {
                continuation.read_registers(group, bytes)?
            }
            _ => unreachable!(),
        }
        super::info::output(caller, args[3], args[4], bytes)
    })
}

fn with_bytes<T>(
    length: usize,
    operation: impl FnOnce(&mut [u8]) -> Result<T, CallError>,
) -> Result<T, CallError> {
    const SCALAR_SIZE: usize = abi::register::ScalarFloat::LOONGARCH_SIZE;
    if length <= SCALAR_SIZE {
        let mut bytes = [0; SCALAR_SIZE];
        operation(&mut bytes[..length])
    } else {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|_| MemoryError::NoMemory)?;
        bytes.resize(length, 0);
        operation(&mut bytes)
    }
}
