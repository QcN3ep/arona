// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Authorized snapshots of object information.

use super::error::CallError;
use crate::{
    task::{ObjectReference, Rights, Task, TaskState},
    thread::ThreadState,
};
use abi::{Error, info, thread::Lifecycle};

pub(super) fn invoke(
    caller: &Task,
    target: &ObjectReference,
    rights: Rights,
    args: [u64; 5],
) -> Result<[u64; 2], CallError> {
    if let ObjectReference::Task(target) = target
        && args[0] == info::task::ADDRESS_SPACE
    {
        return address_space(caller, target, args);
    }
    if matches!(target, ObjectReference::SystemInfo(_))
        && args[0] == info::system::RUNNABLE_INTEGRAL
    {
        if args[1..3] != [0; 2] {
            return Err(Error::InvalidArgument.into());
        }
        let snapshot = crate::thread::scheduler::runnable_integral()?;
        return output(caller, args[3], args[4], &snapshot.encode());
    }
    if args[1..] != [0; 4] {
        return Err(Error::InvalidArgument.into());
    }
    Ok(match (target, args[0]) {
        (ObjectReference::Clock(_), info::clock::MONOTONIC) => [
            crate::hardware::timer()
                .now_ns()
                .map_err(|_| Error::InvalidState)?,
            0,
        ],
        (ObjectReference::SystemInfo(_), info::system::MEMORY) => memory_stats(),
        (_, info::HANDLE) => [target.kind() as u64, rights.bits()],
        (ObjectReference::Task(task), info::task::LIFECYCLE) => match task.state() {
            TaskState::Dead(status) => [Lifecycle::Exited as u64, status as u64],
            _ => [Lifecycle::Live as u64, 0],
        },
        (ObjectReference::Thread(thread), info::thread::LIFECYCLE) => match thread.state() {
            ThreadState::Dead(status) => [Lifecycle::Exited as u64, status as u64],
            _ => [Lifecycle::Live as u64, 0],
        },
        (ObjectReference::Thread(thread), info::thread::SCHEDULING) => {
            [u64::from(thread.weight()), 0]
        }
        (ObjectReference::Thread(thread), info::thread::IDENTITY) => [thread.id(), 0],
        (ObjectReference::Thread(thread), info::thread::CPU_TIME) => [thread.cpu_time_ns()?, 0],
        (ObjectReference::FrameAllocation(frames), info::memory::SIZE) => [frames.info().frames, 0],
        (ObjectReference::FrameAllocation(frames), info::memory::USAGE) => {
            [frames.info().mappings, frames.handle_owners()]
        }
        (ObjectReference::KernelLog(_), info::kernel_log::CAPACITY) => {
            [crate::logger::CAPACITY as u64, 0]
        }
        (ObjectReference::DmaDomain(domain), info::dma::STATE) => [domain.state()? as u64, 0],
        _ => return Err(Error::Unsupported.into()),
    })
}

pub(super) fn output(
    caller: &Task,
    address: u64,
    capacity: u64,
    bytes: &[u8],
) -> Result<[u64; 2], CallError> {
    if capacity < bytes.len() as u64 {
        return Err(CallError {
            code: Error::BufferTooSmall,
            values: [bytes.len() as u64, 0],
        });
    }
    if !bytes.is_empty() {
        caller
            .pin_user_memory(address as usize, bytes.len(), true)?
            .write(0, bytes)?;
    }
    Ok([bytes.len() as u64, 0])
}

fn address_space(caller: &Task, target: &Task, args: [u64; 5]) -> Result<[u64; 2], CallError> {
    use abi::memory::{AddressMode, AddressQuery, AddressRegion, RegionKind};
    let query = AddressQuery::decode(&self::query(caller, args[1], args[2])?)?;
    let region = match query.mode {
        AddressMode::Bounds => {
            let range = target.address_space()?.bounds();
            AddressRegion {
                start: range.start as u64,
                end: range.end as u64,
                kind: RegionKind::Bounds,
                permissions: None,
                leaf_level: 0,
            }
        }
        AddressMode::Containing | AddressMode::AtOrAfter => {
            let found = if query.mode == AddressMode::Containing {
                target.query(query.address as usize)?
            } else {
                target.query_next(query.address as usize)?
            };
            let Some(region) = found else {
                return Ok([0; 2]);
            };
            AddressRegion {
                start: region.range.start as u64,
                end: region.range.end as u64,
                kind: if region.mapping.is_some() {
                    RegionKind::Mapping
                } else {
                    RegionKind::Reserved
                },
                permissions: region.mapping.map(|mapping| {
                    super::memory::native_permissions(mapping.attributes.permissions)
                }),
                leaf_level: region.leaf_level as u64,
            }
        }
    };
    output(caller, args[3], args[4], &region.encode())
}

pub(super) fn query<const N: usize>(
    caller: &Task,
    address: u64,
    length: u64,
) -> Result<[u8; N], CallError> {
    if length != N as u64 {
        return Err(Error::InvalidArgument.into());
    }
    let mut bytes = [0; N];
    caller
        .pin_user_memory(address as usize, N, false)?
        .read(0, &mut bytes)?;
    Ok(bytes)
}

/// Total and free page-frame bytes, including reserved and metadata frames in the total.
fn memory_stats() -> [u64; 2] {
    let stats = crate::memory::manager().stats();
    let page = crate::hardware::mmu::PAGE_SIZE as u64;
    let total = stats
        .kernel
        .total_frames
        .saturating_add(stats.user.total_frames)
        .saturating_add(stats.reserved_frames)
        .saturating_add(stats.metadata_frames)
        .saturating_mul(page);
    let free = stats
        .kernel
        .free_frames
        .saturating_add(stats.user.free_frames)
        .saturating_mul(page);
    [total, free]
}
