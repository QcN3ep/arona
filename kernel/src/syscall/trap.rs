// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::error::CallError;
use crate::{
    task::{Object, ObjectReference, Task},
    trap::port::{TrapContinuationReference, TrapPortReference},
};
use abi::{Error, Handle, ObjectType, Rights, method, trap::TrapInfo};

pub(super) fn task(
    caller: &Task,
    target: &Task,
    method: u64,
    args: [u64; 5],
) -> Result<[u64; 2], CallError> {
    match method {
        method::task::TRAP_PORT => {
            let rights = Rights::from_bits(args[0]).ok_or(Error::InvalidArgument)?;
            if !ObjectType::TrapPort.supported_rights().contains(rights) {
                return Err(Error::InvalidArgument.into());
            }
            let [handle] = super::object::create(caller, [rights], || {
                Ok([Object::TrapPort(target.create_trap_port()?)])
            })?;
            Ok([handle.raw(), 0])
        }
        method::task::SET_TRAP => {
            let ObjectReference::TrapPort(port) = caller.get(
                Handle::from_raw(args[0]),
                ObjectType::TrapPort,
                Rights::READ,
            )?
            else {
                unreachable!()
            };
            target.bind_trap(&port)?;
            Ok([0; 2])
        }
        _ => Err(Error::Unsupported.into()),
    }
}

pub(super) fn receive(
    caller: &Task,
    port: &TrapPortReference,
    args: [u64; 5],
) -> Result<[u64; 2], CallError> {
    let delivery = port.try_receive()?;
    let output = caller.pin_user_memory(args[0] as usize, TrapInfo::SIZE, true)?;
    let rights = Rights::READ | Rights::SET_STATE | Rights::TRANSFER;
    let [handle] = super::object::create(caller, [rights], || {
        output.write(0, &delivery.info().encode())?;
        Ok([Object::TrapContinuation(delivery.commit())])
    })?;
    Ok([handle.raw(), TrapInfo::SIZE as u64])
}

pub(super) fn continuation(
    caller: &Task,
    target: &TrapContinuationReference,
    method: u64,
    args: [u64; 5],
) -> Result<[u64; 2], CallError> {
    match method {
        method::trap_continuation::RESUME => {
            super::register::apply(
                caller,
                args,
                |group| target.register_size(group),
                |groups| target.resume_registers(groups),
            )?;
            Ok([0; 2])
        }
        _ => Err(Error::Unsupported.into()),
    }
}
