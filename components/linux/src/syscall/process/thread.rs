// SPDX-License-Identifier: MPL-2.0

//! Linux thread creation and thread identifiers.

// TODO: Integrate robust futex registration and thread exit cleanup.

use crate::{
    process::{Process, Thread},
    syscall::{
        Context,
        abi::{EFAULT, EINVAL, ENOMEM, EOPNOTSUPP, SyscallResult},
    },
};
use runtime::{Rights, thread::UserEntry};

const VM: usize = 0x100;
const FS: usize = 0x200;
const FILES: usize = 0x400;
const SIGHAND: usize = 0x800;
const THREAD: usize = 0x10000;
const SYSVSEM: usize = 0x40000;
const SETTLS: usize = 0x80000;
const PARENT_SETTID: usize = 0x100000;
const CHILD_CLEARTID: usize = 0x200000;
const CHILD_SETTID: usize = 0x1000000;

// REFERENCE: https://man7.org/linux/man-pages/man2/clone.2.html
pub(super) fn clone(
    process: &mut Process<'_>,
    index: usize,
    context: &Context,
    args: [usize; 6],
) -> SyscallResult {
    let [flags, stack, parent_tid, tls, child_tid] = arguments(process.machine, args);
    let required = VM | FS | FILES | SIGHAND | THREAD;
    let supported = required | SYSVSEM | SETTLS | PARENT_SETTID | CHILD_CLEARTID | CHILD_SETTID;
    if flags & required != required || flags & !supported != 0 {
        return Err(EOPNOTSUPP);
    }
    if stack == 0 || stack & 15 != 0 {
        return Err(EINVAL);
    }
    process.threads.try_reserve(1).map_err(|_| ENOMEM)?;
    let tid = process.namespace.allocate().ok_or(ENOMEM)?;
    let mut state = context.state.clone();
    process
        .machine
        .complete(&mut state, context.trap.info.successor as usize, Ok(0));
    process
        .machine
        .thread_state(&mut state, stack, (flags & SETTLS != 0).then_some(tls));
    let native = process
        .address_space
        .native
        .create_thread(
            UserEntry {
                pc: state.pc as u64,
                stack_pointer: stack as u64,
                thread_pointer: 0,
                argument: 0,
            },
            Rights::WAIT
                | Rights::START
                | Rights::SET_STATE
                | Rights::TERMINATE
                | Rights::INTERRUPT,
        )
        .map_err(super::error)?;
    let setup = start(
        process,
        &native,
        &state,
        flags,
        [tid, parent_tid, child_tid],
    );
    match setup {
        Ok(native_id) => {
            let signals = alloc::rc::Rc::new(core::cell::RefCell::new(
                crate::process::signal::ThreadSignals {
                    mask: process.threads[index].signals.borrow().mask,
                    ..Default::default()
                },
            ));
            process
                .namespace
                .register_thread(process.pid, tid, &signals);
            process.threads.push(Thread {
                native,
                native_id,
                tid,
                clear_tid: if flags & CHILD_CLEARTID != 0 {
                    child_tid
                } else {
                    0
                },
                pending: None,
                signals,
                held: None,
                interrupt_requested: false,
            });
            Ok(tid)
        }
        Err(error) => {
            let _ = native.terminate(0);
            Err(error)
        }
    }
}

fn tid_writes(
    process: &Process<'_>,
    flags: usize,
    parent_tid: usize,
    child_tid: usize,
) -> Result<alloc::vec::Vec<(usize, [u8; 4])>, crate::syscall::abi::Errno> {
    let mut writes = alloc::vec::Vec::new();
    for address in [
        (flags & PARENT_SETTID != 0).then_some(parent_tid),
        (flags & CHILD_SETTID != 0).then_some(child_tid),
    ]
    .into_iter()
    .flatten()
    {
        let mut previous = [0; 4];
        process
            .address_space
            .read(address, &mut previous)
            .map_err(|_| EFAULT)?;
        writes.push((address, previous));
    }
    Ok(writes)
}

fn arguments(machine: crate::elf::Machine, args: [usize; 6]) -> [usize; 5] {
    // REFERENCE: https://git.musl-libc.org/cgit/musl/tree/src/thread/loongarch64/clone.s
    let [flags, stack, parent_tid, fourth, fifth, _] = args;
    let (tls, child_tid) = match machine {
        crate::elf::Machine::Riscv64 => (fourth, fifth),
        crate::elf::Machine::LoongArch64 => (fifth, fourth),
    };
    [flags, stack, parent_tid, tls, child_tid]
}

fn start(
    process: &Process<'_>,
    native: &runtime::Handle<runtime::Thread>,
    state: &runtime::trap::UserState,
    flags: usize,
    ids: [usize; 3],
) -> Result<u64, crate::syscall::abi::Errno> {
    let [tid, parent_tid, child_tid] = ids;
    native.set_user_state(state).map_err(super::error)?;
    let native_id = native.id().map_err(super::error)?;
    let writes = tid_writes(process, flags, parent_tid, child_tid)?;
    for (index, (address, _)) in writes.iter().enumerate() {
        if process
            .address_space
            .write(*address, &(tid as u32).to_le_bytes())
            .is_err()
        {
            for (address, previous) in &writes[..index] {
                let _ = process.address_space.write(*address, previous);
            }
            return Err(EFAULT);
        }
    }
    if let Err(error) = native.start() {
        for (address, previous) in writes {
            let _ = process.address_space.write(address, &previous);
        }
        return Err(super::error(error));
    }
    Ok(native_id)
}
