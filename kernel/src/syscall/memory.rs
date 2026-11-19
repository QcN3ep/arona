// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::error::CallError;
use crate::{
    hardware::mmu::PagePermissions,
    memory::{
        self as mm, FrameAllocation, HeapArc, MappingOptions, MappingRights, MemoryBuffer,
        VirtRange,
    },
    task::{Object, ObjectReference, ObjectType, Task},
};
use abi::{
    Error, Handle, Rights,
    method::{memory, task},
};

pub(super) fn permissions(raw: u64) -> Result<PagePermissions, CallError> {
    match abi::memory::Permissions::from_raw(raw).ok_or(Error::InvalidArgument)? {
        abi::memory::Permissions::NoAccess => Ok(PagePermissions::NoAccess),
        abi::memory::Permissions::ReadOnly => Ok(PagePermissions::ReadOnly),
        abi::memory::Permissions::ReadWrite => Ok(PagePermissions::ReadWrite),
        abi::memory::Permissions::ReadExecute => Ok(PagePermissions::ReadExecute),
        abi::memory::Permissions::ReadWriteExecute => Ok(PagePermissions::ReadWriteExecute),
        abi::memory::Permissions::ExecuteOnly => Ok(PagePermissions::ExecuteOnly),
    }
}

pub(super) fn native_permissions(permissions: PagePermissions) -> abi::memory::Permissions {
    use abi::memory::Permissions;
    match permissions {
        PagePermissions::NoAccess => Permissions::NoAccess,
        PagePermissions::ReadOnly => Permissions::ReadOnly,
        PagePermissions::ReadWrite => Permissions::ReadWrite,
        PagePermissions::ReadExecute => Permissions::ReadExecute,
        PagePermissions::ReadWriteExecute => Permissions::ReadWriteExecute,
        PagePermissions::ExecuteOnly => Permissions::ExecuteOnly,
    }
}

pub(super) fn range(start: u64, length: u64) -> Result<VirtRange, CallError> {
    Ok(VirtRange {
        start: start as usize,
        end: start.checked_add(length).ok_or(Error::InvalidArgument)? as usize,
    })
}

pub(super) fn task(
    caller: &Task,
    target: &Task,
    method: u64,
    args: [u64; 5],
) -> Result<[u64; 2], CallError> {
    match method {
        task::ALLOCATE => allocate(caller, target, args),
        task::READ_MEMORY | task::WRITE_MEMORY => copy_task_memory(caller, target, method, args),
        task::ATOMIC_COMPARE_EXCHANGE => atomic_compare_exchange(target, args),
        task::MAP => map(caller, target, args),

        task::UNMAP => {
            target.unmap(range(args[0], args[1])?)?;
            Ok([0; 2])
        }
        task::PROTECT => {
            if args[3..] != [0; 2] {
                return Err(Error::InvalidArgument.into());
            }
            target.protect(range(args[0], args[1])?, permissions(args[2])?)?;
            Ok([0; 2])
        }
        task::RESERVE => {
            if args[3..] != [0; 2] {
                return Err(Error::InvalidArgument.into());
            }
            let range = target.reserve(
                args[0] as usize,
                args[1] as usize,
                (args[2] != 0).then_some(args[2] as usize),
            )?;
            Ok([range.start as u64, (range.end - range.start) as u64])
        }
        task::UNRESERVE => {
            target.unreserve(range(args[0], args[1])?)?;
            Ok([0; 2])
        }
        _ => Err(Error::Unsupported.into()),
    }
}

fn copy_task_memory(
    caller: &Task,
    target: &Task,
    method: u64,
    args: [u64; 5],
) -> Result<[u64; 2], CallError> {
    let write = method == task::WRITE_MEMORY;
    if write && caller.same(target) {
        return Err(Error::Denied.into());
    }
    let length = usize::try_from(args[2]).map_err(|_| Error::InvalidArgument)?;
    let remote = target.pin_user_memory(args[0] as usize, length, write)?;
    let local = caller.pin_user_memory(args[1] as usize, length, !write)?;
    let result = if write {
        local.copy_to(&remote)
    } else {
        remote.copy_to(&local)
    };
    result.map_err(mm::UserMemoryError)?;
    Ok([args[2], 0])
}

fn atomic_compare_exchange(target: &Task, args: [u64; 5]) -> Result<[u64; 2], CallError> {
    let expected = u32::try_from(args[1]).map_err(|_| Error::InvalidArgument)?;
    let desired = u32::try_from(args[2]).map_err(|_| Error::InvalidArgument)?;
    let remote = target.pin_user_memory(args[0] as usize, 4, true)?;
    let old = remote
        .compare_exchange_u32(0, expected, desired)
        .map_err(mm::UserMemoryError)?;
    Ok([
        u64::from(old.unwrap_or(expected)),
        u64::from(old.is_some() as u8),
    ])
}

fn allocate(caller: &Task, target: &Task, args: [u64; 5]) -> Result<[u64; 2], CallError> {
    let rights = Rights::from_bits(args[1]).ok_or(Error::InvalidArgument)?;
    if !ObjectType::FrameAllocation
        .supported_rights()
        .contains(rights)
    {
        return Err(Error::InvalidArgument.into());
    }
    let [handle] = super::object::create(caller, [rights], || {
        let allocation = if args[2] == 0 {
            target.allocate_frames(args[0])?
        } else {
            target.allocate_frames_aligned(args[0], args[2])?
        };
        Ok([Object::frame_allocation(
            mm::manager().allocate_arc(allocation)?,
        )?])
    })?;
    Ok([handle.raw(), 0])
}

fn map(caller: &Task, target: &Task, args: [u64; 5]) -> Result<[u64; 2], CallError> {
    let mode = abi::memory::MapMode::decode(args[4]).ok_or(Error::InvalidArgument)?;
    let level = u64::from(mode.leaf_level);
    if level > 1 {
        return Err(Error::InvalidArgument.into());
    }
    let (ObjectReference::FrameAllocation(allocation), options) =
        mapping_reference(caller, args[0], mode.permissions)?
    else {
        unreachable!()
    };
    let frames = args[1]..args[1].checked_add(args[2]).ok_or(Error::InvalidArgument)?;
    let range = match (level, args[3]) {
        (0, 0) => target.map(&allocation, frames, options)?,
        (0, address) => target.map_at(address as usize, &allocation, frames, options)?,
        (1, 0) => target.map_huge(&allocation, frames, options)?,
        (1, address) => target.map_huge_at(address as usize, &allocation, frames, options)?,
        _ => unreachable!(),
    };
    Ok([range.start as u64, (range.end - range.start) as u64])
}

fn mapping_reference(
    caller: &Task,
    raw: u64,
    native: abi::memory::Permissions,
) -> Result<(ObjectReference, MappingOptions), CallError> {
    let permissions = permissions(native as u64)?;
    let handle = Handle::from_raw(raw);
    let (object, rights) = caller.with_handles(|table| table.lookup(handle))??;
    let ObjectReference::FrameAllocation(allocation) = object else {
        return Err(Error::WrongType.into());
    };
    let mut maximum = MappingRights::from(permissions);
    for (right, mapping) in [
        (Rights::READ, MappingRights::READ),
        (Rights::WRITE, MappingRights::WRITE),
        (Rights::EXECUTE, MappingRights::EXECUTE),
    ] {
        if rights.contains(right) {
            maximum = maximum | mapping;
        }
    }
    if !rights.contains(native.mapping_rights()) {
        return Err(Error::Denied.into());
    }
    let options = MappingOptions {
        maximum,
        ..MappingOptions::new(permissions)
    };
    Ok((ObjectReference::FrameAllocation(allocation), options))
}

pub(super) fn frames(
    caller: &Task,
    allocation: &HeapArc<FrameAllocation>,
    method: u64,
    args: [u64; 5],
) -> Result<[u64; 2], CallError> {
    let write = method == memory::WRITE;
    let user = caller.pin_user_memory(args[1] as usize, args[2] as usize, !write)?;
    let data = MemoryBuffer::allocation(allocation, args[0] as usize, args[2] as usize, write)?;
    let mut bytes = [0; 256];
    for offset in (0..args[2] as usize).step_by(bytes.len()) {
        let length = bytes.len().min(args[2] as usize - offset);
        if write {
            user.read(offset, &mut bytes[..length])?;
            data.write(offset, &bytes[..length])?;
        } else {
            data.read(offset, &mut bytes[..length])?;
            user.write(offset, &bytes[..length])?;
        }
    }
    Ok([args[2], 0])
}
