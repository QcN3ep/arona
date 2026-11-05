// SPDX-License-Identifier: MPL-2.0

use crate::{
    task::{Object, ObjectReference, Task},
    trap::port::{TrapContinuationReference, TrapPortReference},
};
use abi::{
    Error, Handle, ObjectType, Rights, method,
    trap::{TrapInfo, UserState},
};

pub(super) fn task(
    caller: &Task,
    target: &Task,
    method: u64,
    args: [u64; 5],
) -> Result<[u64; 2], Error> {
    match method {
        method::task::TRAP_PORT => {
            let rights = Rights::from_bits(args[0]).ok_or(Error::InvalidArgument)?;
            if !ObjectType::TrapPort.supported_rights().contains(rights) {
                return Err(Error::InvalidArgument);
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
        _ => Err(Error::Unsupported),
    }
}

pub(super) fn receive(
    caller: &Task,
    port: &TrapPortReference,
    args: [u64; 5],
) -> Result<[u64; 2], Error> {
    let output = caller.pin_user_memory(args[0] as usize, TrapInfo::SIZE, true)?;
    let delivery = port.try_receive()?;
    output.write(0, &delivery.info().encode())?;
    let rights = Rights::READ | Rights::SET_STATE | Rights::TRANSFER;
    let [handle] = super::object::create(caller, [rights], || {
        Ok([Object::TrapContinuation(delivery.commit())])
    })?;
    Ok([handle.raw(), TrapInfo::SIZE as u64])
}

pub(super) fn continuation(
    caller: &Task,
    target: &TrapContinuationReference,
    method: u64,
    args: [u64; 5],
) -> Result<[u64; 2], Error> {
    match method {
        method::trap_continuation::STATE => {
            let output = caller.pin_user_memory(args[0] as usize, UserState::SIZE, true)?;
            output.write(0, &target.user_state()?.encode())?;
            Ok([UserState::SIZE as u64, 0])
        }
        method::trap_continuation::RESUME => {
            target.resume(&read_state(caller, args[0])?)?;
            Ok([0; 2])
        }
        _ => Err(Error::Unsupported),
    }
}

pub(super) fn read_state(caller: &Task, address: u64) -> Result<UserState, Error> {
    let input = caller.pin_user_memory(address as usize, UserState::SIZE, false)?;
    let mut bytes = [0; UserState::SIZE];
    input.read(0, &mut bytes)?;
    Ok(UserState::decode(&bytes))
}
