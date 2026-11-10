// SPDX-License-Identifier: MPL-2.0

mod thread;

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
    let result = match number {
        abi::GETPID => Ok(process.pid),
        abi::GETPPID => Ok(process.parent),
        abi::GETTID => Ok(process.threads[index].tid),
        abi::SET_TID_ADDRESS => {
            process.threads[index].clear_tid = args[0];
            Ok(process.threads[index].tid)
        }
        abi::CLONE => thread::clone(process, context, args),
        abi::SCHED_YIELD => runtime::thread::yield_now().map(|()| 0).map_err(error),
        _ => return None,
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
