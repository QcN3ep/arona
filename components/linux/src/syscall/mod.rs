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

use crate::{Error, elf::Machine, process::Process};

use abi::{ENOSYS, SyscallResult};

struct Context {
    machine: Machine,
    trap: Trap,
    state: UserState,
}

pub(crate) struct Pending<'a> {
    context: Context,
    operation: Operation<'a>,
}

enum Outcome<'a> {
    Complete(SyscallResult),
    Pending(Operation<'a>),
    Terminated,
}

enum Operation<'a> {
    File(file::Pending<'a>),
    Memory(memory::Pending<'a>),
}

impl<'a> Operation<'a> {
    fn call(&self) -> Option<&Handle<Call>> {
        match self {
            Self::File(operation) => operation.call(),
            Self::Memory(operation) => operation.call(),
        }
    }
    fn poll(&mut self, process: &mut Process<'a>, files: &fs::Client<'a>) -> Option<SyscallResult> {
        match self {
            Self::File(operation) => operation.poll(process, files),
            Self::Memory(operation) => operation.poll(process),
        }
    }
}

pub(crate) fn dispatch<'a>(
    process: &mut Process<'a>,
    index: usize,
    trap: Trap,
    files: &fs::Client<'a>,
) -> Result<(), Error> {
    let context = Context {
        machine: process.machine,
        state: trap.state()?,
        trap,
    };
    let (number, arguments) = context.machine.syscall(&context.state);
    let outcome = if let Some(outcome) = file::dispatch(process, files, number, arguments) {
        outcome
    } else {
        match number {
            abi::EXIT => process::exit(process, index, arguments[0])?,
            abi::EXIT_GROUP => process::exit_group(process, arguments[0])?,
            abi::BRK => Outcome::Complete(Ok(process.address_space.brk(arguments[0])?)),
            abi::MMAP => memory::mmap(process, arguments),
            abi::MADVISE => Outcome::Complete(memory::madvise(&process.address_space, arguments)),
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
        }
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

pub(crate) fn poll<'a>(
    process: &mut Process<'a>,
    index: usize,
    files: &fs::Client<'a>,
) -> Result<bool, Error> {
    let Some(mut pending) = process.threads[index].pending.take() else {
        return Ok(false);
    };
    let Some(result) = pending.operation.poll(process, files) else {
        process.threads[index].pending = Some(pending);
        return Ok(false);
    };
    pending.context.complete(result)?;
    Ok(true)
}

impl Pending<'_> {
    pub fn call(&self) -> Option<&Handle<Call>> {
        self.operation.call()
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
