// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Linux syscall dispatch and asynchronous completion.

// TODO: Support epoll and signalfd.

pub(crate) mod abi;
mod file;
mod memory;
mod process;
mod signal;
mod system;
mod wait;

use alloc::boxed::Box;
use runtime::trap::{Trap, UserState};

use crate::{Error, elf::Machine, process::Process};

use abi::{ENOSYS, SyscallResult};

pub(crate) struct Context {
    machine: Machine,
    pub(crate) trap: Trap,
    pub(crate) state: UserState,
}

#[derive(Clone, Copy)]
pub(crate) struct Restart {
    arguments: [usize; 6],
    deadline: (u64, bool),
}

pub(crate) struct Pending<'a> {
    context: Context,
    operation: Operation<'a>,
}

enum Outcome<'a> {
    Complete(SyscallResult),
    Pending(Operation<'a>),
    Vfork {
        wait: alloc::rc::Rc<crate::process::Vfork>,
    },
    Terminated,
    Restored,
    Restart,
}

enum Operation<'a> {
    Exec(Box<process::exec::Pending<'a>>),
    WaitChild(process::wait::Pending),
    Futex(process::futex::Pending),
    Signal(signal::Pending),
    File(file::Pending<'a>),
    Memory(memory::Pending<'a>),
    System(system::Pending),
}

impl<'a> Operation<'a> {
    fn waits<'b>(&'b self, items: &mut alloc::vec::Vec<runtime::ipc::WaitItem<'b>>) {
        match self {
            Self::Exec(operation) => {
                if let Some(call) = operation.call() {
                    items.push(runtime::ipc::WaitItem::new(call, runtime::ipc::READABLE));
                }
            }
            Self::WaitChild(_) => {}
            Self::Signal(operation) => operation.waits(items),
            Self::Futex(operation) => operation.waits(items),
            Self::File(operation) => operation.waits(items),
            Self::System(operation) => operation.waits(items),
            Self::Memory(operation) => {
                if let Some(call) = operation.call() {
                    items.push(runtime::ipc::WaitItem::new(call, runtime::ipc::READABLE));
                }
            }
        }
    }
    fn poll(
        &mut self,
        process: &mut Process<'a>,
        index: usize,
        files: &fs::Client<'a>,
    ) -> Option<SyscallResult> {
        match self {
            Self::Exec(_) => unreachable!(),
            Self::WaitChild(operation) => operation.poll(process),
            Self::Signal(operation) => operation.poll(process, index),
            Self::Futex(operation) => operation.poll(process),
            Self::File(operation) => operation.poll(process, files),
            Self::System(operation) => operation.poll(process),
            Self::Memory(operation) => operation.poll(process),
        }
    }
}

pub(crate) fn dispatch<'a>(
    process: &mut Process<'a>,
    index: usize,
    trap: Trap,
    files: &fs::Client<'a>,
    services: &crate::Services<'_>,
) -> Result<(), Error> {
    let mut context = Context {
        machine: process.machine,
        state: trap.state()?,
        trap,
    };
    let (number, arguments) = context.machine.syscall(&context.state);
    if !matches!(number, abi::FUTEX | abi::RT_SIGRETURN) {
        process.threads[index].signals.borrow_mut().restart = None;
    }
    let mask = signal::temporary_mask(process, index, number, arguments);
    let outcome = if let Err(error) = mask {
        Outcome::Complete(Err(error))
    } else if let Some(outcome) = signal::dispatch(process, index, &mut context, number, arguments)
    {
        outcome
    } else if let Some(outcome) = file::dispatch(process, index, files, services, number, arguments)
    {
        outcome
    } else if let Some(outcome) = system::dispatch(process, services, number, arguments) {
        outcome
    } else if let Some(outcome) =
        process::dispatch(process, index, &context, number, arguments, services)
    {
        outcome
    } else {
        match number {
            abi::EXECVE => process::exec::start(process, files, arguments),
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
            abi::MSYNC => Outcome::Complete(memory::msync(
                &process.address_space,
                arguments[0],
                arguments[1],
                arguments[2],
            )),
            _ => Outcome::Complete(Err(ENOSYS)),
        }
    };
    match outcome {
        Outcome::Complete(result) => context.complete(process, index, result),
        Outcome::Restored => crate::process::signal::delivery::resume(process, index, context),
        Outcome::Restart => {
            if crate::process::signal::delivery::next(process, index).is_some_and(|(_, action)| {
                action.handler != 0 && action.flags & crate::process::signal::RESTART == 0
            }) {
                context.complete(process, index, Err(abi::EINTR))
            } else {
                crate::process::signal::delivery::resume(process, index, context)
            }
        }
        Outcome::Pending(operation) => {
            process.threads[index].pending = Some(Pending { context, operation });
            Ok(())
        }
        Outcome::Vfork { wait } => {
            // The parent thread resumes only after the child execs or exits.
            process.threads[index].vfork = Some(wait);
            process.threads[index].held = Some(context);
            Ok(())
        }
        Outcome::Terminated => Ok(()),
    }
}

pub(crate) fn poll<'a>(
    process: &mut Process<'a>,
    index: usize,
    files: &fs::Client<'a>,
    services: &crate::Services<'_>,
) -> Result<bool, Error> {
    let Some(mut pending) = process.threads[index].pending.take() else {
        return Ok(false);
    };
    let selected = crate::process::signal::delivery::next(process, index);
    if let Some((_, action)) = selected
        && let Some((restartable, result)) = pending.operation.interrupt(process)
    {
        let restart = match &pending.operation {
            Operation::Futex(futex) => futex.restart_info(),
            _ => None,
        };
        let context = pending.context;
        drop(pending.operation);
        if restartable && action.flags & crate::process::signal::RESTART != 0 {
            let arguments = context.machine.syscall(&context.state).1;
            process.threads[index].signals.borrow_mut().restart = restart.map(|deadline| Restart {
                arguments,
                deadline,
            });
            crate::process::signal::delivery::resume(process, index, context)?;
        } else {
            context.complete(process, index, result)?;
        }
        return Ok(true);
    }
    if let Operation::Exec(exec) = &mut pending.operation {
        match exec.poll(process, index, files, services) {
            None => {
                process.threads[index].pending = Some(pending);
                return Ok(false);
            }
            Some(Err(error)) => pending.context.complete(process, index, Err(error))?,
            Some(Ok(())) => {}
        }
        return Ok(true);
    }
    let Some(result) = pending.operation.poll(process, index, files) else {
        let progress =
            selected.is_none() && crate::process::signal::delivery::next(process, index).is_some();
        process.threads[index].pending = Some(pending);
        return Ok(progress);
    };
    pending.context.complete(process, index, result)?;
    Ok(true)
}

impl Pending<'_> {
    pub fn waits<'b>(&'b self, items: &mut alloc::vec::Vec<runtime::ipc::WaitItem<'b>>) {
        self.operation.waits(items);
    }
}

impl Context {
    pub(crate) fn new(machine: Machine, trap: Trap, state: UserState) -> Self {
        Self {
            machine,
            trap,
            state,
        }
    }

    pub(crate) fn complete(
        mut self,
        process: &mut Process<'_>,
        index: usize,
        result: SyscallResult,
    ) -> Result<(), Error> {
        if matches!(result, Err(error) if error == abi::EPIPE) {
            process.threads[index]
                .signals
                .borrow_mut()
                .pending
                .push(crate::process::signal::Info {
                    number: 13,
                    code: 0,
                    sender: process.pid,
                    address: 0,
                    value: 0,
                })
                .map_err(|_| Error::NoMemory)?;
        }
        self.machine
            .complete(&mut self.state, self.trap.info.successor as usize, result);
        crate::process::signal::delivery::resume(process, index, self)
    }
}

impl Operation<'_> {
    fn interrupt(&mut self, process: &Process<'_>) -> Option<(bool, SyscallResult)> {
        let result = match self {
            Self::WaitChild(_) => return Some((true, Err(abi::EINTR))),
            Self::Futex(futex) => return Some((futex.restartable(), Err(abi::EINTR))),
            Self::Exec(_) | Self::Signal(_) => return Some((false, Err(abi::EINTR))),
            Self::File(file) => return file.interrupt(process),
            Self::System(system) => system.interrupt(process)?,
            Self::Memory(_) => return None,
        };
        Some((false, result))
    }
}
