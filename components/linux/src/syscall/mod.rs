// SPDX-License-Identifier: MPL-2.0

//! Linux 系统调用的分发与异步完成。

mod abi;
mod file;
mod memory;
mod process;

use runtime::{
    Call, Handle,
    trap::{Trap, UserState},
};

use crate::{
    Error,
    elf::Machine,
    process::{Process, Thread},
};

use abi::{ENOSYS, SyscallResult};

struct Context {
    machine: Machine,
    trap: Trap,
    state: UserState,
}

pub(crate) struct Pending {
    context: Context,
    operation: Operation,
}

enum Outcome {
    Complete(SyscallResult),
    Pending(Operation),
    Terminated,
}

enum Operation {
    Write(devmgr::device::Pending<usize, usize>),
}

pub(crate) fn dispatch(process: &mut Process, index: usize, trap: Trap) -> Result<(), Error> {
    let context = Context {
        machine: process.machine,
        state: trap.state()?,
        trap,
    };
    let (number, arguments) = context.machine.syscall(&context.state);
    let outcome = match number {
        abi::WRITE => file::write(process, arguments[0], arguments[1], arguments[2]),
        abi::EXIT => process::exit(process, index, arguments[0])?,
        abi::EXIT_GROUP => process::exit_group(process, arguments[0])?,
        abi::BRK => Outcome::Complete(Ok(process.address_space.brk(arguments[0])?)),
        abi::MMAP => Outcome::Complete(memory::mmap(&process.address_space, arguments)),
        abi::MUNMAP => Outcome::Complete(memory::munmap(
            &process.address_space,
            arguments[0],
            arguments[1],
        )),
        abi::MPROTECT => Outcome::Complete(memory::mprotect(
            &process.address_space,
            arguments[0],
            arguments[1],
            arguments[2],
        )),
        _ => Outcome::Complete(Err(ENOSYS)),
    };
    match outcome {
        Outcome::Complete(result) => context.complete(result),
        Outcome::Pending(operation) => {
            process.threads[index].pending = Some(Pending { context, operation });
            Ok(())
        }
        Outcome::Terminated => Ok(()),
    }
}

pub(crate) fn poll(thread: &mut Thread) -> Result<bool, Error> {
    let Some(pending) = &mut thread.pending else {
        return Ok(false);
    };
    let result = match &mut pending.operation {
        Operation::Write(request) => file::poll_write(request),
    };
    let Some(result) = result else {
        return Ok(false);
    };
    thread.pending.take().unwrap().context.complete(result)?;
    Ok(true)
}

impl Pending {
    pub fn call(&self) -> &Handle<Call> {
        match &self.operation {
            Operation::Write(request) => request.call(),
        }
    }
}

impl Context {
    fn complete(mut self, result: SyscallResult) -> Result<(), Error> {
        self.machine
            .complete(&mut self.state, self.trap.info.successor as usize, result);
        self.trap
            .resume(&self.state)
            .map_err(|(error, _)| error.into())
    }
}
