// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Process image copying, vfork sharing, and child process registration.

use crate::{
    process::{Process, Thread, Vfork},
    syscall::{
        Context, Outcome,
        abi::{EFAULT, ENOMEM, EOPNOTSUPP, Errno},
    },
};
use alloc::rc::Rc;
use runtime::{Rights, thread::UserEntry};

const VM: usize = 0x100;
const VFORK: usize = 0x4000;
const PARENT_SETTID: usize = 0x100000;
const SIGCHLD: usize = 17;

pub(super) fn fork<'a>(
    process: &mut Process<'a>,
    index: usize,
    context: &Context,
    args: [usize; 6],
) -> Outcome<'a> {
    match spawn(process, index, context, args) {
        Ok((_, Some(wait))) => Outcome::Vfork { wait },
        Ok((pid, None)) => Outcome::Complete(Ok(pid)),
        Err(error) => Outcome::Complete(Err(error)),
    }
}

// REFERENCE: https://man7.org/linux/man-pages/man2/vfork.2.html
/// Create a child process and return its identifier and, for vfork, its shared wait state.
fn spawn<'a>(
    process: &mut Process<'a>,
    index: usize,
    context: &Context,
    args: [usize; 6],
) -> Result<(usize, Option<Rc<Vfork>>), Errno> {
    let [flags, stack, parent_tid, _, _, _] = args;
    let vfork = validate(flags, stack)?;
    let (group, session) = process.namespace.identity(process.pid)?;
    process.children.try_reserve(1).map_err(|_| ENOMEM)?;
    let pid = process.namespace.allocate().ok_or(ENOMEM)?;
    let address_space = if vfork {
        process
            .address_space
            .fork_shared(process.supervisor, &process.port)
    } else {
        process
            .address_space
            .fork(process.supervisor, &process.port)
    }
    .map_err(super::error)?;
    let mut state = context.state.clone();
    process
        .machine
        .complete(&mut state, context.trap.info.successor as usize, Ok(0));
    let native = address_space
        .native
        .create_thread(
            entry(process.machine, &state),
            Rights::WAIT
                | Rights::START
                | Rights::SET_STATE
                | Rights::TERMINATE
                | Rights::INTERRUPT,
        )
        .map_err(super::error)?;
    native.set_user_state(&state).map_err(super::error)?;
    let native_id = native.id().map_err(super::error)?;
    start(
        process,
        &native,
        pid,
        (flags & PARENT_SETTID != 0).then_some(parent_tid),
    )?;
    let signals = Rc::new(core::cell::RefCell::new(process.signals.borrow().fork()));
    let thread_signals = Rc::new(core::cell::RefCell::new(
        crate::process::signal::ThreadSignals {
            mask: process.threads[index].signals.borrow().mask,
            altstack: process.threads[index].signals.borrow().altstack,
            ..Default::default()
        },
    ));
    process
        .namespace
        .register(pid, process.pid, group, session, &signals);
    process.namespace.register_thread(pid, pid, &thread_signals);
    let waiting = vfork.then(|| Vfork::new(pid));
    process.children.push(Process {
        namespace: Rc::clone(&process.namespace),
        pid,
        signals,
        signal_return: process.signal_return,
        children: alloc::vec::Vec::new(),
        port: Rc::clone(&process.port),
        supervisor: process.supervisor,
        address_space,
        threads: alloc::vec![Thread {
            native,
            native_id,
            tid: pid,
            clear_tid: 0,
            pending: None,
            signals: thread_signals,
            held: None,
            vfork: None,
            interrupt_requested: false,
        }],
        files: process.files.fork(),
        terminal: Rc::clone(&process.terminal),
        cwd: process.cwd.clone(),
        machine: process.machine,
        vfork: waiting.clone(),
        umask: process.umask,
        cpu_ns: 0,
        status: crate::ExitStatus { code: 0, signal: 0 },
    });
    Ok((pid, waiting))
}

/// Check the fork flags and report whether they request vfork.
fn validate(flags: usize, stack: usize) -> Result<bool, Errno> {
    let requested = flags & (VM | VFORK);
    let vfork = requested == VM | VFORK;
    if requested != 0 && !vfork {
        // A shared address space without vfork needs its own clone semantics.
        return Err(EOPNOTSUPP);
    }
    let supported = PARENT_SETTID | 0xff | if vfork { VM | VFORK } else { 0 };
    if flags & !supported != 0 || !matches!(flags & 255, 0 | SIGCHLD) || stack != 0 {
        // TODO: Support the remaining clone flags.
        return Err(EOPNOTSUPP);
    }
    Ok(vfork)
}

fn entry(machine: crate::elf::Machine, state: &runtime::trap::UserState) -> UserEntry {
    UserEntry {
        pc: state.pc as u64,
        stack_pointer: state.registers[match machine {
            crate::elf::Machine::Riscv64 => 2,
            crate::elf::Machine::LoongArch64 => 3,
        }] as u64,
        thread_pointer: 0,
        argument: 0,
    }
}

fn start(
    process: &Process<'_>,
    native: &runtime::Handle<runtime::Thread>,
    pid: usize,
    parent_tid: Option<usize>,
) -> Result<(), Errno> {
    let previous = if let Some(address) = parent_tid {
        let mut bytes = [0; 4];
        process
            .address_space
            .read(address, &mut bytes)
            .map_err(|_| EFAULT)?;
        process
            .address_space
            .write(address, &(pid as u32).to_le_bytes())
            .map_err(|_| EFAULT)?;
        Some((address, bytes))
    } else {
        None
    };
    if let Err(error) = native.start() {
        if let Some((address, bytes)) = previous {
            let _ = process.address_space.write(address, &bytes);
        }
        return Err(super::error(error));
    }
    Ok(())
}
