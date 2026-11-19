// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::error::CallError;
use crate::{
    task::Task,
    thread::{self as threads, Thread, scheduler},
};
use abi::{Error, method::thread};

pub(super) fn invoke(
    caller: &Task,
    target: &Thread,
    method: u64,
    args: [u64; 5],
) -> Result<[u64; 2], CallError> {
    match method {
        thread::SET_STATE => {
            target.set_user_state(&super::trap::read_state(caller, args[0])?)?;
            Ok([0; 2])
        }
        thread::INTERRUPT => {
            target.interrupt()?;
            Ok([0; 2])
        }
        thread::START => {
            target.start()?;
            Ok([0; 2])
        }
        thread::TERMINATE => {
            if is_current(target) {
                scheduler::terminate(target, args[0] as usize);
            } else {
                target.terminate(args[0] as usize)?;
            }
            Ok([0; 2])
        }
        thread::GET_SCHEDULING => Ok([target.scheduling() as u64, 0]),
        thread::SET_SCHEDULING => {
            let policy =
                abi::thread::SchedulingPolicy::from_raw(args[0]).ok_or(Error::InvalidArgument)?;
            target.set_scheduling(policy)?;
            Ok([0; 2])
        }
        thread::GET_PRIORITY => Ok([target.nice() as i64 as u64, 0]),
        thread::SET_PRIORITY => {
            let value = i32::try_from(args[0] as i64).map_err(|_| Error::InvalidArgument)?;
            let nice = i8::try_from(value).map_err(|_| Error::InvalidArgument)?;
            target.set_nice(nice)?;
            Ok([0; 2])
        }
        thread::YIELD | thread::WAIT_MANY => {
            if !is_current(target) {
                return Err(Error::Denied.into());
            }
            match method {
                thread::WAIT_MANY => {
                    return super::ipc::wait_many(&target.task().ok_or(Error::InvalidState)?, args);
                }
                thread::YIELD => threads::yield_now()?,
                _ => unreachable!(),
            }
            Ok([0; 2])
        }
        _ => Err(Error::Unsupported.into()),
    }
}
fn is_current(target: &Thread) -> bool {
    threads::current().is_some_and(|current| current.same(target))
}
