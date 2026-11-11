// SPDX-License-Identifier: MPL-2.0

//! Authorization and dispatch of object calls.

mod device;
mod dma;
mod error;
mod ipc;
mod logger;
mod memory;
mod object;
mod task;
mod thread;
mod trap;

use crate::{
    task::{Object, ObjectReference},
    thread as threads,
    trap::TrapOutcome,
};
use abi::{Error, Handle, Return, Rights, method};

pub(crate) fn dispatch(handle: usize, arguments: [usize; 6]) -> TrapOutcome {
    let result = invoke(
        Handle::from_raw(handle as u64),
        arguments.map(|value| value as u64),
    );
    let result = result.unwrap_or_else(Return::failure);
    TrapOutcome::Return {
        status: result.status as usize,
        values: result.values.map(|value| value as usize),
    }
}

fn invoke(handle: Handle, arguments: [u64; 6]) -> Result<Return, Error> {
    let current = threads::current().ok_or(Error::InvalidState)?;
    let caller = current.task().ok_or(Error::InvalidState)?;
    let self_rights = Rights::DUPLICATE | Rights::TRANSFER | Rights::WAIT | Rights::TERMINATE;
    let (kind, rights) = if handle == Handle::CURRENT_THREAD {
        (abi::ObjectType::Thread, self_rights)
    } else {
        caller.handle_info(handle)?
    };
    let required = method::required_rights(kind, arguments[0]).ok_or(Error::Unsupported)?;
    if !rights.contains(required) {
        return Err(Error::Denied);
    }
    let values = match arguments[0] {
        method::INFO => [kind as u64, rights.bits()],
        method::CLOSE => {
            if handle == Handle::CURRENT_THREAD {
                return Err(Error::InvalidArgument);
            }
            caller.close(handle)?;
            [0; 2]
        }
        method::DUPLICATE => {
            let rights = Rights::from_bits(arguments[1]).ok_or(Error::InvalidArgument)?;
            let duplicate = if handle == Handle::CURRENT_THREAD {
                if !self_rights.contains(rights) {
                    return Err(Error::Denied);
                }
                caller.insert(Object::Thread(current.clone()), rights)?
            } else {
                caller.duplicate(handle, rights)?
            };
            [duplicate.raw(), 0]
        }
        _ => {
            let object = if handle == Handle::CURRENT_THREAD {
                ObjectReference::Thread(current)
            } else {
                caller.get(handle, kind, required)?
            };
            let args = arguments[1..].try_into().unwrap();
            match object {
                ObjectReference::DeviceSet(target) => {
                    device::catalog(&caller, &target, arguments[0], args)?
                }
                ObjectReference::Device(target) => {
                    device::invoke(&caller, &target, arguments[0], args)?
                }
                ObjectReference::DmaDomain(target) => {
                    dma::invoke(&caller, &target, arguments[0], args)?
                }
                ObjectReference::Task(target) => {
                    task::invoke(&caller, &target, rights, arguments[0], args)?
                }
                ObjectReference::Thread(target) => {
                    thread::invoke(&caller, &target, arguments[0], args)?
                }
                ObjectReference::TrapPort(target) => trap::receive(&caller, &target, args)?,
                ObjectReference::TrapContinuation(target) => {
                    trap::continuation(&caller, &target, arguments[0], args)?
                }
                ObjectReference::FrameAllocation(allocation) => {
                    memory::frames(&caller, &allocation, arguments[0], args)?
                }
                ObjectReference::KernelLog(target) => logger::read(&caller, &target, args)?,
                object => return ipc::invoke(&caller, handle, object, arguments[0], args),
            }
        }
    };
    Ok(Return::success(values))
}
