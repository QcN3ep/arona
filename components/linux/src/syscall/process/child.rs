// SPDX-License-Identifier: MPL-2.0

//! Process image copying and child process registration.

use crate::{
    process::{Process, Thread},
    syscall::{
        Context,
        abi::{EFAULT, ENOMEM, EOPNOTSUPP, SyscallResult},
    },
};
use alloc::rc::Rc;
use runtime::{Rights, thread::UserEntry};

pub(super) fn fork(
    process: &mut Process<'_>,
    index: usize,
    context: &Context,
    args: [usize; 6],
) -> SyscallResult {
    let [flags, stack, parent_tid, _, _, _] = args;
    if flags & !0x1000ff != 0 || !matches!(flags & 255, 0 | 17) || stack != 0 {
        // TODO: Support vfork's shared address space and parent thread suspension.
        return Err(EOPNOTSUPP);
    }
    let (group, session) = process.namespace.identity(process.pid)?;
    process.children.try_reserve(1).map_err(|_| ENOMEM)?;
    let pid = process.namespace.allocate().ok_or(ENOMEM)?;
    let address_space = process
        .address_space
        .fork(process.supervisor, &process.port)
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
        (flags & 0x100000 != 0).then_some(parent_tid),
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
            interrupt_requested: false,
        }],
        files: process.files.fork(),
        terminal: Rc::clone(&process.terminal),
        cwd: process.cwd.clone(),
        machine: process.machine,
        umask: process.umask,
        status: crate::ExitStatus { code: 0, signal: 0 },
    });
    Ok(pid)
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
) -> Result<(), crate::syscall::abi::Errno> {
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
