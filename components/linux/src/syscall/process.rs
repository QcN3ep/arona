// SPDX-License-Identifier: MPL-2.0

mod child;
pub(super) mod exec;
pub(super) mod futex;
mod identity;
mod thread;
pub(super) mod wait;

use super::{
    Context, Outcome,
    abi::{self, EIO, ENOMEM, Errno},
};

pub(super) fn dispatch<'a>(
    process: &mut Process<'a>,
    index: usize,
    context: &Context,
    number: usize,
    args: [usize; 6],
) -> Option<Outcome<'a>> {
    if number == abi::FUTEX {
        return Some(futex::start(process, args));
    }
    if matches!(number, abi::WAIT4 | abi::WAITID) {
        return Some(wait::start(process, number, args));
    }
    let result = match number {
        abi::GETPID => Ok(process.pid),
        abi::GETPPID => Ok(process.namespace.parent(process.pid)),
        abi::GETTID => Ok(process.threads[index].tid),
        abi::SET_TID_ADDRESS => {
            process.threads[index].clear_tid = args[0];
            Ok(process.threads[index].tid)
        }
        abi::CLONE if args[0] & 0x10000 == 0 => child::fork(process, index, context, args),
        abi::CLONE => thread::clone(process, index, context, args),
        abi::SCHED_YIELD => runtime::thread::yield_now().map(|()| 0).map_err(error),
        _ => identity::dispatch(process, number, args)?,
    };
    Some(Outcome::Complete(result))
}

fn error(error: runtime::Error) -> Errno {
    match error {
        runtime::Error::Kernel(runtime::KernelError::NoMemory) => ENOMEM,
        _ => EIO,
    }
}
use crate::{Error, ExitStatus, process::Process};

pub(super) fn exit<'a>(
    process: &Process<'a>,
    index: usize,
    code: usize,
) -> Result<Outcome<'a>, Error> {
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
