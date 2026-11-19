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
        thread::SET_SCHEDULING => {
            let weight = u32::try_from(args[0]).map_err(|_| Error::InvalidArgument)?;
            if args[1..] != [0; 4] {
                return Err(Error::InvalidArgument.into());
            }
            target.set_weight(weight)?;
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
