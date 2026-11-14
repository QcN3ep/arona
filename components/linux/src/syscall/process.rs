// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

mod child;
pub(super) mod exec;
pub(super) mod futex;
mod identity;
mod rlimit;
mod thread;
pub(super) mod wait;

use super::{
    Context, Outcome,
    abi::{self, EFAULT, EINVAL, EIO, ENOMEM, ESRCH, Errno, SyscallResult},
};

pub(super) fn dispatch<'a>(
    process: &mut Process<'a>,
    index: usize,
    context: &Context,
    number: usize,
    args: [usize; 6],
    services: &crate::Services<'_>,
) -> Option<Outcome<'a>> {
    if number == abi::FUTEX {
        return Some(futex::start(process, index, services, args));
    }
    if matches!(number, abi::WAIT4 | abi::WAITID) {
        return Some(wait::start(process, number, args));
    }
    if number == abi::PRLIMIT64 {
        return Some(rlimit::start(process, args));
    }
    let result = match number {
        abi::GETPID => Ok(process.pid),
        abi::GETPPID => Ok(process.namespace.parent(process.pid)),
        abi::GETTID => Ok(process.threads[index].tid),
        abi::SET_TID_ADDRESS => {
            process.threads[index].clear_tid = args[0];
            Ok(process.threads[index].tid)
        }
        abi::SET_ROBUST_LIST => set_robust_list(process, index, args),
        abi::GET_ROBUST_LIST => get_robust_list(process, index, args),
        abi::CLONE if args[0] & 0x10000 == 0 => {
            return Some(child::fork(process, index, context, args));
        }
        abi::CLONE => thread::clone(process, index, context, args),
        abi::SCHED_YIELD => runtime::thread::yield_now().map(|()| 0).map_err(error),
        _ => identity::dispatch(process, number, args)?,
    };
    Some(Outcome::Complete(result))
}

/// Register the calling thread's `struct robust_list_head`.
fn set_robust_list(process: &mut Process<'_>, index: usize, args: [usize; 6]) -> SyscallResult {
    let [head, length, ..] = args;
    if length != ROBUST_LIST_HEAD_SIZE {
        return Err(EINVAL);
    }
    if head != 0 {
        let mut header = [0; ROBUST_LIST_HEAD_SIZE];
        process
            .address_space
            .read(head, &mut header)
            .map_err(|_| EFAULT)?;
    }
    process.threads[index].robust_list = head;
    Ok(0)
}

/// Report a thread's robust list head and length.
fn get_robust_list(process: &Process<'_>, index: usize, args: [usize; 6]) -> SyscallResult {
    let [pid, head_ptr, length_ptr, ..] = args;
    let pid = pid as i32;
    let thread = if pid == 0 {
        &process.threads[index]
    } else if pid > 0 {
        process
            .threads
            .iter()
            .find(|thread| thread.tid == pid as usize)
            .ok_or(ESRCH)?
    } else {
        return Err(ESRCH);
    };
    process
        .address_space
        .write(length_ptr, &(ROBUST_LIST_HEAD_SIZE as u64).to_le_bytes())
        .map_err(|_| EFAULT)?;
    process
        .address_space
        .write(head_ptr, &(thread.robust_list as u64).to_le_bytes())
        .map_err(|_| EFAULT)?;
    Ok(0)
}

fn error(error: runtime::Error) -> Errno {
    match error {
        runtime::Error::Kernel(runtime::KernelError::NoMemory) => ENOMEM,
        _ => EIO,
    }
}
use crate::{
    Error, ExitStatus,
    process::{Process, futex::ROBUST_LIST_HEAD_SIZE},
};

pub(super) fn exit<'a>(
    process: &Process<'a>,
    index: usize,
    code: usize,
) -> Result<Outcome<'a>, Error> {
    if process.threads.len() == 1 {
        process.release_vfork();
    }
    process.threads[index].native.terminate(
        ExitStatus {
            code: code as u8,
            signal: 0,
        }
        .encode(),
    )?;
    Ok(Outcome::Terminated)
}

pub(super) fn exit_group<'a>(process: &Process<'a>, code: usize) -> Result<Outcome<'a>, Error> {
    process.release_vfork();
    let status = ExitStatus {
        code: code as u8,
        signal: 0,
    }
    .encode();
    for thread in &process.threads {
        thread.native.terminate(status)?;
    }
    Ok(Outcome::Terminated)
}
