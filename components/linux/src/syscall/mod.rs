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
    operation: file::Pending<'a>,
}

enum Outcome<'a> {
    Complete(SyscallResult),
    Pending(file::Pending<'a>),
    Terminated,
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
    let outcome = match number {
        abi::DUP => Outcome::Complete(file::duplicate(process, arguments[0], 0, false)),
        abi::DUP3 => Outcome::Complete(file::dup3(
            process,
            arguments[0],
            arguments[1],
            arguments[2],
        )),
        abi::FCNTL => Outcome::Complete(file::fcntl(
            process,
            arguments[0],
            arguments[1],
            arguments[2],
        )),
        abi::OPENAT => file::openat(
            process,
            files,
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        ),
        abi::CLOSE => Outcome::Complete(file::close(process, arguments[0])),
        abi::LSEEK => file::seek(process, arguments[0], arguments[1], arguments[2]),
        abi::READ | abi::PREAD64 | abi::READV | abi::PREADV => file::read(
            process,
            arguments[0],
            io_buffer(number, &arguments),
            matches!(number, abi::PREAD64 | abi::PREADV).then_some(arguments[3]),
        ),
        abi::WRITE | abi::PWRITE64 | abi::WRITEV | abi::PWRITEV => file::write(
            process,
            arguments[0],
            io_buffer(number, &arguments),
            matches!(number, abi::PWRITE64 | abi::PWRITEV).then_some(arguments[3]),
        ),
        abi::FTRUNCATE => file::truncate(process, arguments[0], arguments[1]),
        abi::FSYNC | abi::FDATASYNC => file::sync(process, arguments[0]),
        abi::UMASK => Outcome::Complete(Ok(core::mem::replace(
            &mut process.umask,
            arguments[0] as u16 & 0o777,
        ) as usize)),
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

fn io_buffer(number: usize, arguments: &[usize; 6]) -> file::Buffer {
    if matches!(
        number,
        abi::READV | abi::PREADV | abi::WRITEV | abi::PWRITEV
    ) {
        file::Buffer::Vector {
            address: arguments[1],
            count: arguments[2],
        }
    } else {
        file::Buffer::Slice {
            address: arguments[1],
            length: arguments[2],
        }
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
