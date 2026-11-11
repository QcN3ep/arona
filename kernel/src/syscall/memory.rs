// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use crate::{
    hardware::mmu::PagePermissions,
    memory::{self as mm, FrameAllocation, MappingOptions, MappingRights, MemoryBuffer, VirtRange},
    task::{Object, ObjectReference, ObjectType, Task},
};
use abi::{
    Error, Handle, Rights,
    method::{memory, task},
};

pub(super) fn permissions(raw: u64) -> Result<PagePermissions, Error> {
    match abi::memory::Permissions::from_raw(raw).ok_or(Error::InvalidArgument)? {
        abi::memory::Permissions::NoAccess => Ok(PagePermissions::NoAccess),
        abi::memory::Permissions::ReadOnly => Ok(PagePermissions::ReadOnly),
        abi::memory::Permissions::ReadWrite => Ok(PagePermissions::ReadWrite),
        abi::memory::Permissions::ReadExecute => Ok(PagePermissions::ReadExecute),
        abi::memory::Permissions::ExecuteOnly => Ok(PagePermissions::ExecuteOnly),
    }
}

pub(super) fn range(start: u64, length: u64) -> Result<VirtRange, Error> {
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
) -> Result<[u64; 2], Error> {
    match method {
        task::ADDRESS_RANGE => {
            let bounds = target.address_space()?.bounds();
            Ok([bounds.start as u64, bounds.end as u64])
        }
        task::ALLOCATE => allocate(caller, target, args),
        task::READ_MEMORY | task::WRITE_MEMORY => copy_task_memory(caller, target, method, args),
        task::MAP => map(caller, target, args),
        task::UNMAP => {
            target.unmap(range(args[0], args[1])?)?;
            Ok([0; 2])
        }
        task::PROTECT => {
            target.protect(range(args[0], args[1])?, permissions(args[2])?)?;
            Ok([0; 2])
        }
        task::RESERVE => {
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
        task::QUERY | task::QUERY_NEXT => {
            let info = if method == task::QUERY_NEXT {
                target.query_next(args[0] as usize)?
            } else {
                target.query(args[0] as usize)?
            };
            let Some(info) = info else {
                return Ok([0; 2]);
            };
            let permission =
                info.mapping
                    .map_or(0, |mapping| match mapping.attributes.permissions {
                        PagePermissions::NoAccess => 8,
                        PagePermissions::ReadOnly => 1,
                        PagePermissions::ReadWrite => 3,
                        PagePermissions::ExecuteOnly => 4,
                        PagePermissions::ReadExecute => 5,
                    });
            Ok([info.range.start as u64, info.range.end as u64 | permission])
        }
        _ => Err(Error::Unsupported),
    }
}

fn copy_task_memory(
    caller: &Task,
    target: &Task,
    method: u64,
    args: [u64; 5],
) -> Result<[u64; 2], Error> {
    let write = method == task::WRITE_MEMORY;
    if write && caller.same(target) {
        return Err(Error::Denied);
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

fn allocate(caller: &Task, target: &Task, args: [u64; 5]) -> Result<[u64; 2], Error> {
    let rights = Rights::from_bits(args[1]).ok_or(Error::InvalidArgument)?;
    if !ObjectType::FrameAllocation
        .supported_rights()
        .contains(rights)
    {
        return Err(Error::InvalidArgument);
    }
    let [handle] = super::object::create(caller, [rights], || {
        let allocation = target.allocate_frames(args[0])?;
        Ok([Object::FrameAllocation(
            mm::manager().allocate_arc(allocation)?,
        )])
    })?;
    Ok([handle.raw(), 0])
}

fn map(caller: &Task, target: &Task, args: [u64; 5]) -> Result<[u64; 2], Error> {
    let handle = Handle::from_raw(args[0]);
    let (kind, rights) = caller.handle_info(handle)?;
    if kind != ObjectType::FrameAllocation {
        return Err(Error::WrongType);
    }
    let permissions = permissions(args[4])?;
    let mut required = Rights::MAP;
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
    match permissions {
        PagePermissions::NoAccess => {}
        PagePermissions::ReadOnly => required = required | Rights::READ,
        PagePermissions::ReadWrite => required = required | Rights::READ | Rights::WRITE,
        PagePermissions::ReadExecute => required = required | Rights::READ | Rights::EXECUTE,
        PagePermissions::ExecuteOnly => required = required | Rights::EXECUTE,
    }
    let ObjectReference::FrameAllocation(allocation) = caller.get(handle, kind, required)? else {
        unreachable!()
    };
    let frames = args[1]..args[1].checked_add(args[2]).ok_or(Error::InvalidArgument)?;
    let options = MappingOptions {
        maximum,
        ..MappingOptions::new(permissions)
    };
    let range = if args[3] == 0 {
        target.map(&allocation, frames, options)?
    } else {
        target.map_at(args[3] as usize, &allocation, frames, options)?
    };
    Ok([range.start as u64, (range.end - range.start) as u64])
}

pub(super) fn frames(
    caller: &Task,
    allocation: &FrameAllocation,
    method: u64,
    args: [u64; 5],
) -> Result<[u64; 2], Error> {
    if method == memory::SIZE {
        return Ok([allocation.info().frames, 0]);
    }
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
