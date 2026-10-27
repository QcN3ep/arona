// SPDX-License-Identifier: MPL-2.0

use crate::{
    hardware::execution::UserEntry,
    task::{Object, ObjectType, Task, TaskState, TransferRequest},
};
use abi::{Error, Rights, method::task};

const CREATION_RIGHTS: Rights = Rights::CREATE_TASK
    .union(Rights::CREATE_THREAD)
    .union(Rights::FRAME_ALLOCATE)
    .union(Rights::MAP_MEMORY)
    .union(Rights::INSTALL_HANDLE)
    .union(Rights::CREATE_CHANNEL)
    .union(Rights::CREATE_EVENT)
    .union(Rights::CREATE_TIMER);

pub(super) fn invoke(
    caller: &Task,
    target: &Task,
    authority: Rights,
    method: u64,
    args: [u64; 5],
) -> Result<[u64; 2], Error> {
    match method {
        task::STATE => Ok(match target.state() {
            TaskState::Created => [0, 0],
            TaskState::Active => [1, 0],
            TaskState::Dying(status) => [2, status as u64],
            TaskState::Dead(status) => [3, status as u64],
        }),
        task::CREATE => {
            let rights = Rights::from_bits(args[0]).ok_or(Error::InvalidArgument)?;
            if !ObjectType::Task.supported_rights().contains(rights) {
                return Err(Error::InvalidArgument);
            }
            if rights.bits() & CREATION_RIGHTS.bits() & !authority.bits() != 0 {
                return Err(Error::Denied);
            }
            let [handle] = super::object::create(caller, [rights], || {
                Ok([Object::Task(target.create_child()?)])
            })?;
            Ok([handle.raw(), 0])
        }
        task::CREATE_THREAD => create_thread(caller, target, args),
        task::INSTALL => Ok([
            target
                .install_handle(
                    caller,
                    TransferRequest {
                        handle: abi::Handle::from_raw(args[0]),
                        rights: Rights::from_bits(args[1]).ok_or(Error::InvalidArgument)?,
                    },
                )?
                .raw(),
            0,
        ]),
        task::TERMINATE => {
            target.terminate(args[0] as usize);
            Ok([0; 2])
        }
        task::WAIT => {
            let status = if args[0] == u64::MAX {
                Some(target.wait()?)
            } else {
                target.wait_until(args[0])?
            };
            Ok([status.ok_or(Error::TimedOut)? as u64, 0])
        }
        task::CHANNEL | task::EVENT | task::TIMER => {
            super::ipc::create(caller, target, method, args)
        }
        _ => super::memory::task(caller, target, method, args),
    }
}
fn create_thread(caller: &Task, target: &Task, args: [u64; 5]) -> Result<[u64; 2], Error> {
    let rights = Rights::from_bits(args[1]).ok_or(Error::InvalidArgument)?;
    if !ObjectType::Thread.supported_rights().contains(rights) {
        return Err(Error::InvalidArgument);
    }
    let input = caller.pin_user_memory(args[0] as usize, abi::thread::UserEntry::SIZE, false)?;
    let mut bytes = [0; abi::thread::UserEntry::SIZE];
    input.read(0, &mut bytes)?;
    let entry = abi::thread::UserEntry::decode(&bytes);
    let [handle] = super::object::create(caller, [rights], || {
        Ok([Object::Thread(target.create_thread(UserEntry {
            pc: entry.pc as usize,
            stack_pointer: entry.stack_pointer as usize,
            thread_pointer: entry.thread_pointer as usize,
            argument: entry.argument as usize,
        })?)])
    })?;
    Ok([handle.raw(), 0])
}
