// SPDX-License-Identifier: MPL-2.0

use runtime::trap::{Trap, UserState};

use crate::{
    Error, ExitStatus,
    elf::Machine,
    process::{Process, Thread},
};

pub(crate) struct PendingWrite {
    pub request: devmgr::device::Pending<usize, usize>,
    trap: Trap,
    state: UserState,
}

const EIO: isize = -5;
const EBADF: isize = -9;
const EFAULT: isize = -14;
const EINVAL: isize = -22;
const ENOSYS: isize = -38;

// REFERENCE: https://github.com/torvalds/linux/blob/master/include/uapi/asm-generic/unistd.h
pub(crate) fn dispatch(process: &mut Process, index: usize, trap: Trap) -> Result<(), Error> {
    let state = trap.state()?;
    let (number, arguments) = process.machine.syscall(&state);
    match number {
        64 => write(process, index, trap, state, arguments),
        93 | 94 => {
            let status = ExitStatus {
                code: arguments[0] as u8,
                signal: 0,
            }
            .encode();
            if number == 94 {
                for thread in &process.threads {
                    thread.native.terminate(status)?;
                }
            } else {
                process.threads[index].native.terminate(status)?;
            }
            Ok(())
        }
        214 => {
            let address = process.address_space.brk(arguments[0])?;
            complete(process.machine, trap, state, address as isize)
        }
        _ => complete(process.machine, trap, state, ENOSYS),
    }
}

fn write(
    process: &mut Process,
    index: usize,
    trap: Trap,
    state: UserState,
    arguments: [usize; 6],
) -> Result<(), Error> {
    let [fd, address, length, ..] = arguments;
    let Some(file) = process.files.get(&fd) else {
        return complete(process.machine, trap, state, EBADF);
    };
    if length > isize::MAX as usize {
        return complete(process.machine, trap, state, EINVAL);
    }
    if length == 0 {
        return complete(process.machine, trap, state, 0);
    }
    let mut buffer = [0; devmgr::uart::MAX_BYTES];
    let buffer = &mut buffer[..length.min(devmgr::uart::MAX_BYTES)];
    if process.address_space.read(address, buffer).is_err() {
        return complete(process.machine, trap, state, EFAULT);
    }
    match file.output.start_write(buffer) {
        Ok(request) => {
            process.threads[index].pending = Some(PendingWrite {
                request,
                trap,
                state,
            });
            Ok(())
        }
        Err(_) => complete(process.machine, trap, state, EIO),
    }
}

pub(crate) fn poll(machine: Machine, thread: &mut Thread) -> Result<bool, Error> {
    let Some(pending) = &mut thread.pending else {
        return Ok(false);
    };
    let result = match pending.request.try_complete() {
        Ok(length) => length as isize,
        Err(devmgr::Error::Runtime(runtime::Error::Kernel(runtime::KernelError::WouldBlock))) => {
            return Ok(false);
        }
        Err(_) => EIO,
    };
    let pending = thread.pending.take().unwrap();
    complete(machine, pending.trap, pending.state, result)?;
    Ok(true)
}

fn complete(
    machine: Machine,
    trap: Trap,
    mut state: UserState,
    result: isize,
) -> Result<(), Error> {
    machine.complete(&mut state, trap.info.successor as usize, result);
    trap.resume(&state).map_err(|(error, _)| error.into())
}
