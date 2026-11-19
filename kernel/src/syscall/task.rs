// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::error::CallError;
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
) -> Result<[u64; 2], CallError> {
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
                return Err(Error::InvalidArgument.into());
            }
            if rights.bits() & CREATION_RIGHTS.bits() & !authority.bits() != 0 {
                return Err(Error::Denied.into());
            }
            let [handle] = super::object::create(caller, [rights], || {
                Ok([Object::Task(target.create_child()?)])
            })?;
            Ok([handle.raw(), 0])
        }
        task::RANDOM => {
            let length = usize::try_from(args[1]).map_err(|_| Error::InvalidArgument)?;
            let mut bytes = [0; 256];
            let output = bytes.get_mut(..length).ok_or(Error::InvalidArgument)?;
            let memory = caller.pin_user_memory(args[0] as usize, length, true)?;
            crate::random::fill(output)?;
            memory.write(0, output)?;
            Ok([0; 2])
        }
        task::CREATE_THREAD => create_thread(caller, target, args),
        task::TRAP_PORT | task::SET_TRAP => super::trap::task(caller, target, method, args),
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
        // System-wide statistics are readable through any Task handle; they expose only totals.
        task::MEMORY_STATS => Ok(memory_stats()),
        // Pack the 16.16 fixed-point 1-minute and 5-minute averages into one value.
        task::LOAD_AVERAGE => {
            let [one, five, fifteen] = crate::thread::scheduler::load_average();
            Ok([(u64::from(one) << 32) | u64::from(five), u64::from(fifteen)])
        }
        _ => super::memory::task(caller, target, method, args),
    }
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

fn create_thread(caller: &Task, target: &Task, args: [u64; 5]) -> Result<[u64; 2], CallError> {
    let rights = Rights::from_bits(args[1]).ok_or(Error::InvalidArgument)?;
    if !ObjectType::Thread.supported_rights().contains(rights) {
        return Err(Error::InvalidArgument.into());
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
