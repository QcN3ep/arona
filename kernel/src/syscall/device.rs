// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::error::CallError;
use crate::{
    device::{DeviceReference, DeviceSet},
    task::{Object, ObjectReference, Task},
};
use abi::{
    Error, Handle, ObjectType, Rights,
    method::{device, device_set},
};

pub(super) fn catalog(
    caller: &Task,
    target: &DeviceSet,
    method: u64,
    args: [u64; 5],
) -> Result<[u64; 2], CallError> {
    let id = u32::try_from(args[0]).map_err(|_| Error::InvalidArgument)?;
    match method {
        device_set::CLAIM_GROUP => {
            let rights = Rights::from_bits(args[1]).ok_or(Error::InvalidArgument)?;
            if !ObjectType::Device.supported_rights().contains(rights) {
                return Err(Error::Denied.into());
            }
            let [handle] = super::object::create(caller, [rights], || {
                Ok([Object::Device(target.claim_group(id)?)])
            })?;
            Ok([handle.raw(), 0])
        }
        _ => Err(Error::Unsupported.into()),
    }
}

pub(super) fn invoke(
    caller: &Task,
    target: &DeviceReference,
    method: u64,
    args: [u64; 5],
) -> Result<[u64; 2], CallError> {
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
        device::CREATE_DMA => {
            let rights = Rights::from_bits(args[1]).ok_or(Error::InvalidArgument)?;
            if !ObjectType::DmaDomain.supported_rights().contains(rights) {
                return Err(Error::Denied.into());
            }
            let [handle] = super::object::create(caller, [rights], || {
                Ok([Object::DmaDomain(target.create_dma(id)?)])
            })?;
            Ok([handle.raw(), 0])
        }
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
        _ => Err(Error::Unsupported.into()),
    }
}

pub(super) fn info(
    caller: &Task,
    target: &ObjectReference,
    args: [u64; 5],
) -> Result<[u64; 2], CallError> {
    use abi::info::device as topic;
    let indexed = matches!(
        args[0],
        topic::IDENTITY_NAMESPACE
            | topic::IDENTITY
            | topic::PROPERTY_NAMESPACE
            | topic::PROPERTY_NAME
            | topic::PROPERTY_VALUE
    );
    let (id, index) = if indexed {
        let bytes = super::info::query::<16>(caller, args[1], args[2])?;
        (
            u64::from_le_bytes(bytes[..8].try_into().unwrap()),
            u64::from_le_bytes(bytes[8..].try_into().unwrap()),
        )
    } else {
        (
            u64::from_le_bytes(super::info::query(caller, args[1], args[2])?),
            0,
        )
    };
    let id = u32::try_from(id).map_err(|_| Error::InvalidArgument)?;
    if args[0] == topic::NEXT_DEVICE {
        if args[3..] != [0; 2] {
            return Err(Error::InvalidArgument.into());
        }
        let next = match target {
            ObjectReference::DeviceSet(set) => set.next(id)?,
            ObjectReference::Device(grant) => grant.next(id)?,
            _ => return Err(Error::Unsupported.into()),
        };
        return Ok([u64::from(next.unwrap_or(0)), 0]);
    }
    if args[0] == topic::DEVICE {
        let info = match target {
            ObjectReference::DeviceSet(set) => set.describe(id)?,
            ObjectReference::Device(grant) => grant.describe(id)?,
            _ => return Err(Error::Unsupported.into()),
        };
        return super::info::output(caller, args[3], args[4], &info.encode());
    }
    let index = usize::try_from(index).map_err(|_| Error::InvalidArgument)?;
    let bytes = match target {
        ObjectReference::DeviceSet(set) => set.read(id, args[0], index)?,
        ObjectReference::Device(grant) => grant.read(id, args[0], index)?,
        _ => return Err(Error::Unsupported.into()),
    };
    super::info::output(caller, args[3], args[4], bytes)
}
