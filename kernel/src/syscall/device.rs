// SPDX-License-Identifier: MPL-2.0

use crate::{
    device::{DeviceReference, DeviceSet},
    task::{Object, ObjectReference, Task},
};
use abi::{
    Error, Handle, ObjectType, Rights,
    device::DeviceField,
    method::{device, device_set},
};

pub(super) fn catalog(
    caller: &Task,
    target: &DeviceSet,
    method: u64,
    args: [u64; 5],
) -> Result<[u64; 2], Error> {
    let id = u32::try_from(args[0]).map_err(|_| Error::InvalidArgument)?;
    match method {
        device_set::NEXT => Ok([target.next(id)?.unwrap_or(0) as u64, 0]),
        device_set::DESCRIBE => {
            let bytes = target.describe(id)?.encode();
            write(caller, args[1], args[2], &bytes)?;
            Ok([bytes.len() as u64, 0])
        }
        device_set::READ => {
            let field = DeviceField::from_raw(args[1]).ok_or(Error::InvalidArgument)?;
            let bytes = target.read(id, field, args[2] as usize)?;
            write(caller, args[3], args[4], bytes)?;
            Ok([bytes.len() as u64, 0])
        }
        device_set::CLAIM_GROUP => {
            let rights = Rights::from_bits(args[1]).ok_or(Error::InvalidArgument)?;
            if !ObjectType::Device.supported_rights().contains(rights) {
                return Err(Error::Denied);
            }
            let [handle] = super::object::create(caller, [rights], || {
                Ok([Object::Device(target.claim_group(id)?)])
            })?;
            Ok([handle.raw(), 0])
        }
        _ => Err(Error::Unsupported),
    }
}

pub(super) fn invoke(
    caller: &Task,
    target: &DeviceReference,
    method: u64,
    args: [u64; 5],
) -> Result<[u64; 2], Error> {
    if method == device::REVOKE {
        target.revoke()?;
        return Ok([0; 2]);
    }
    if method == device::UNMAP {
        target.unmap(caller, args[0] as usize)?;
        return Ok([0; 2]);
    }
    let id = u32::try_from(args[0]).map_err(|_| Error::InvalidArgument)?;
    let index = args[1] as usize;
    match method {
        device::MAP => {
            let range = target.map(caller, id, index)?;
            Ok([range.start as u64, (range.end - range.start) as u64])
        }
        device::BIND => {
            let ObjectReference::Event(event) =
                caller.get(Handle::from_raw(args[2]), ObjectType::Event, Rights::SIGNAL)?
            else {
                unreachable!("invalid event handle type");
            };
            target.bind(id, index, event, args[3])?;
            Ok([0; 2])
        }
        device::UNBIND => {
            target.unbind(id, index)?;
            Ok([0; 2])
        }
        device::ACK => {
            target.acknowledge(id, index)?;
            Ok([0; 2])
        }
        _ => Err(Error::Unsupported),
    }
}

fn write(caller: &Task, address: u64, capacity: u64, bytes: &[u8]) -> Result<(), Error> {
    if capacity < bytes.len() as u64 {
        return Err(Error::BufferTooSmall);
    }
    caller
        .pin_user_memory(address as usize, bytes.len(), true)?
        .write(0, bytes)?;
    Ok(())
}
