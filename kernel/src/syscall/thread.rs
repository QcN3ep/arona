// SPDX-License-Identifier: MPL-2.0

use crate::{
    hardware,
    thread::{self as threads, Thread, ThreadState, scheduler},
};
use abi::{Error, method::thread};

pub(super) fn invoke(target: &Thread, method: u64, args: [u64; 5]) -> Result<[u64; 2], Error> {
    match method {
        thread::STATE => Ok(match target.state() {
            ThreadState::Created => [0, 0],
            ThreadState::Ready => [1, 0],
            ThreadState::Running => [2, 0],
            ThreadState::Blocked => [3, 0],
            ThreadState::Dying(status) => [4, status as u64],
            ThreadState::Dead(status) => [5, status as u64],
        }),
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
        thread::WAIT => {
            let status = if args[0] == u64::MAX {
                Some(target.wait()?)
            } else {
                target.wait_until(args[0])?
            };
            Ok([status.ok_or(Error::TimedOut)? as u64, 0])
        }
        thread::NOW => Ok([
            hardware::timer()
                .now_ns()
                .map_err(|_| Error::InvalidState)?,
            0,
        ]),
        thread::EXIT | thread::YIELD | thread::SLEEP | thread::WAIT_MANY => {
            if !is_current(target) {
                return Err(Error::Denied);
            }
            match method {
                thread::WAIT_MANY => {
                    return super::ipc::wait_many(&target.task().ok_or(Error::InvalidState)?, args);
                }
                thread::EXIT => scheduler::terminate(target, args[0] as usize),
                thread::YIELD => threads::yield_now()?,
                thread::SLEEP => threads::sleep_until(args[0])?,
                _ => unreachable!(),
            }
            Ok([0; 2])
        }
        _ => Err(Error::Unsupported),
    }
}
fn is_current(target: &Thread) -> bool {
    threads::current().is_some_and(|current| current.same(target))
}
