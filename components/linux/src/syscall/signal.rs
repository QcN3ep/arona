// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Linux signal syscalls.

use crate::{
    process::{
        Process,
        signal::{self, Action, AltStack, Info, bit, frame},
    },
    syscall::{
        Context, Operation, Outcome,
        abi::{self, EFAULT, EINVAL, ENOMEM, EPERM, ESRCH, Errno, SyscallResult},
        wait::{self, Deadline},
    },
};
use alloc::vec::Vec;
use runtime::{ipc::WaitItem, trap::UserState};

pub(super) enum Pending {
    Suspend,
    Wait {
        mask: u64,
        output: usize,
        deadline: Deadline,
    },
}

// REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/kernel/signal.c?h=v6.12
pub(super) fn dispatch<'a>(
    process: &mut Process<'a>,
    index: usize,
    context: &mut Context,
    number: usize,
    args: [usize; 6],
) -> Option<Outcome<'a>> {
    let result = match number {
        abi::KILL => send(process, number, args),
        abi::TKILL | abi::TGKILL => send(process, number, args),
        abi::RT_SIGACTION => action(process, args),
        abi::RT_SIGPROCMASK => mask(process, index, args),
        abi::RT_SIGPENDING => pending(process, index, args),
        abi::SIGALTSTACK => altstack(process, index, &context.state, args),
        abi::RT_SIGRETURN => return Some(restore(process, index, context)),
        abi::RT_SIGSUSPEND => return Some(suspend(process, index, args)),
        abi::RT_SIGTIMEDWAIT => return Some(timed_wait(process, index, args)),
        _ => return None,
    };
    Some(Outcome::Complete(result))
}

fn send(process: &Process<'_>, call: usize, args: [usize; 6]) -> SyscallResult {
    let group = call == abi::TGKILL;
    let number = if group { args[2] } else { args[1] } as u32;
    if number > 64 {
        return Err(EINVAL);
    }
    let info = Info {
        number: number as u8,
        code: if call == abi::KILL { 0 } else { -6 },
        sender: process.pid,
        uid: process.credentials.uid,
        address: 0,
        value: 0,
    };
    if call != abi::KILL {
        let tid = if group { args[1] } else { args[0] } as u32 as usize;
        return process
            .namespace
            .signal_thread(tid, group.then_some(args[0] as u32 as usize), info)
            .map(|()| 0);
    }
    let selector = args[0] as i32;
    let pids: Vec<_> = process
        .namespace
        .processes
        .borrow()
        .iter()
        .filter_map(|(&pid, record)| {
            let selected = match selector {
                -1 => pid != process.pid,
                0 => record.group == process.namespace.group(process.pid),
                target if target > 0 => pid == target as usize,
                target => record.group == target.unsigned_abs() as usize,
            };
            selected.then_some(pid)
        })
        .collect();
    if pids.is_empty() {
        return Err(ESRCH);
    }
    for pid in pids {
        process.namespace.signal_process(pid, info)?;
    }
    Ok(0)
}

fn action(process: &Process<'_>, args: [usize; 6]) -> SyscallResult {
    let [number, input, output, size, ..] = args;
    if !(1..=64).contains(&number) || input != 0 && matches!(number, 9 | 19) || size != 8 {
        return Err(EINVAL);
    }
    let new = if input == 0 {
        None
    } else {
        let mut bytes = [0; 24];
        process
            .address_space
            .read(input, &mut bytes)
            .map_err(|_| EFAULT)?;
        let flags = word(&bytes, 8);
        if flags
            & !(3 | 4 | signal::ONSTACK | signal::RESTART | signal::NODEFER | signal::RESETHAND)
            != 0
        {
            return Err(EINVAL);
        }
        Some(Action {
            handler: word(&bytes, 0),
            flags,
            mask: word(&bytes, 16) as u64 & !signal::UNBLOCKABLE,
        })
    };
    let old = process.signals.borrow().actions[number - 1];
    if output != 0 {
        let mut bytes = [0; 24];
        for (offset, value) in [(0, old.handler), (8, old.flags), (16, old.mask as usize)] {
            put(&mut bytes, offset, value);
        }
        process
            .address_space
            .write(output, &bytes)
            .map_err(|_| EFAULT)?;
    }
    if let Some(action) = new {
        process.signals.borrow_mut().actions[number - 1] = action;
        if action.handler == 1 {
            process.namespace.discard(process.pid, number as u8);
        }
    }
    Ok(0)
}

fn mask(process: &Process<'_>, index: usize, args: [usize; 6]) -> SyscallResult {
    let [how, input, output, size, ..] = args;
    if size != 8 {
        return Err(EINVAL);
    }
    let new = if input == 0 {
        None
    } else {
        if how > 2 {
            return Err(EINVAL);
        }
        let mut bytes = [0; 8];
        process
            .address_space
            .read(input, &mut bytes)
            .map_err(|_| EFAULT)?;
        Some(u64::from_le_bytes(bytes))
    };
    let mut signals = process.threads[index].signals.borrow_mut();
    if output != 0 {
        process
            .address_space
            .write(output, &signals.mask.to_le_bytes())
            .map_err(|_| EFAULT)?;
    }
    if let Some(value) = new {
        signals.mask = match how {
            0 => signals.mask | value,
            1 => signals.mask & !value,
            _ => value,
        } & !signal::UNBLOCKABLE;
    }
    Ok(0)
}

fn pending(process: &Process<'_>, index: usize, args: [usize; 6]) -> SyscallResult {
    if args[1] != 8 {
        return Err(EINVAL);
    }
    let thread = process.threads[index].signals.borrow();
    let pending = (thread.pending.mask() | process.signals.borrow().pending.mask()) & thread.mask;
    process
        .address_space
        .write(args[0], &pending.to_le_bytes())
        .map_err(|_| EFAULT)?;
    Ok(0)
}

fn altstack(
    process: &Process<'_>,
    index: usize,
    state: &UserState,
    args: [usize; 6],
) -> SyscallResult {
    let old = process.threads[index].signals.borrow().altstack;
    let active = old.contains(frame::stack_pointer(process.machine, state));
    let new = if args[0] == 0 {
        None
    } else {
        if active {
            return Err(EPERM);
        }
        let mut bytes = [0; 24];
        process
            .address_space
            .read(args[0], &mut bytes)
            .map_err(|_| EFAULT)?;
        let flags = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        if flags & !2 != 0 {
            return Err(EINVAL);
        }
        let value = AltStack {
            address: word(&bytes, 0),
            size: word(&bytes, 16),
            disabled: flags == 2,
        };
        if !value.disabled {
            let minimum = match process.machine {
                crate::elf::Machine::Riscv64 => 2048,
                crate::elf::Machine::LoongArch64 => 4096,
            };
            if value.size < minimum {
                return Err(ENOMEM);
            }
            value.address.checked_add(value.size).ok_or(EFAULT)?;
        }
        Some(value)
    };
    if args[1] != 0 {
        let mut bytes = [0; 24];
        put(&mut bytes, 0, old.address);
        put(
            &mut bytes,
            8,
            if old.disabled { 2 } else { usize::from(active) },
        );
        put(&mut bytes, 16, old.size);
        process
            .address_space
            .write(args[1], &bytes)
            .map_err(|_| EFAULT)?;
    }
    if let Some(value) = new {
        process.threads[index].signals.borrow_mut().altstack = value;
    }
    Ok(0)
}

fn suspend<'a>(process: &Process<'a>, index: usize, args: [usize; 6]) -> Outcome<'a> {
    let result = (|| {
        if args[1] != 8 {
            return Err(EINVAL);
        }
        let mut bytes = [0; 8];
        process
            .address_space
            .read(args[0], &mut bytes)
            .map_err(|_| EFAULT)?;
        let mut signals = process.threads[index].signals.borrow_mut();
        signals.suspended_mask = Some(signals.mask);
        signals.mask = u64::from_le_bytes(bytes) & !signal::UNBLOCKABLE;
        Ok(Pending::Suspend)
    })();
    outcome(result)
}

fn timed_wait<'a>(process: &Process<'a>, index: usize, args: [usize; 6]) -> Outcome<'a> {
    let result = (|| {
        if args[3] != 8 {
            return Err(EINVAL);
        }
        let mut bytes = [0; 8];
        process
            .address_space
            .read(args[0], &mut bytes)
            .map_err(|_| EFAULT)?;
        let deadline = Deadline::new(process.supervisor, wait::duration(process, args[2])?)?;
        let mask = u64::from_le_bytes(bytes) & !signal::UNBLOCKABLE;
        let mut thread = process.threads[index].signals.borrow_mut();
        thread.suspended_mask = Some(thread.mask);
        thread.mask |= mask;
        Ok(Pending::Wait {
            mask,
            output: args[1],
            deadline,
        })
    })();
    outcome(result)
}

fn outcome<'a>(pending: Result<Pending, Errno>) -> Outcome<'a> {
    match pending {
        Ok(pending) => Outcome::Pending(Operation::Signal(pending)),
        Err(error) => Outcome::Complete(Err(error)),
    }
}

impl Pending {
    pub fn waits<'a>(&'a self, items: &mut Vec<WaitItem<'a>>) {
        if let Self::Wait { deadline, .. } = self {
            deadline.waits(items);
        }
    }
    pub fn poll(&self, process: &Process<'_>, index: usize) -> Option<SyscallResult> {
        let Self::Wait {
            mask,
            output,
            deadline,
        } = self
        else {
            return None;
        };
        let signal = process.threads[index]
            .signals
            .borrow()
            .pending
            .first(*mask)
            .or_else(|| process.signals.borrow().pending.first(*mask));
        if let Some(info) = signal {
            if *output != 0 {
                let bytes = info.encode();
                if process.address_space.write(*output, &bytes).is_err() {
                    return Some(Err(EFAULT));
                }
            }
            process.threads[index]
                .signals
                .borrow_mut()
                .pending
                .take(bit(info.number))
                .or_else(|| process.signals.borrow_mut().pending.take(bit(info.number)));
            return Some(Ok(info.number as usize));
        }
        match deadline.expired() {
            Ok(true) => Some(Err(abi::EAGAIN)),
            Ok(false) => None,
            Err(error) => Some(Err(error)),
        }
    }
}

fn restore<'a>(process: &mut Process<'a>, index: usize, context: &mut Context) -> Outcome<'a> {
    let frame_address = frame::stack_pointer(process.machine, &context.state);
    match frame::restore(&process.address_space, process.machine, frame_address) {
        Ok(restored) => {
            let mut signals = process.threads[index].signals.borrow_mut();
            signals.restart = signals
                .restart_frames
                .iter()
                .rposition(|(address, _)| *address == frame_address)
                .map(|position| {
                    let restart = signals.restart_frames[position].1;
                    signals.restart_frames.truncate(position);
                    restart
                });
            signals.mask = restored.mask & !signal::UNBLOCKABLE;
            signals.altstack = restored.altstack;
            context.state = restored.state;
            Outcome::Restored
        }
        Err(_) => {
            let _ = crate::process::signal::delivery::terminate(process, 11);
            Outcome::Terminated
        }
    }
}

fn word(bytes: &[u8], offset: usize) -> usize {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap()) as usize
}
fn put(bytes: &mut [u8], offset: usize, value: usize) {
    bytes[offset..offset + 8].copy_from_slice(&(value as u64).to_le_bytes());
}

pub(super) fn temporary_mask(
    process: &Process<'_>,
    index: usize,
    number: usize,
    args: [usize; 6],
) -> Result<(), Errno> {
    let (address, size) = match number {
        abi::PPOLL => (args[3], args[4]),
        abi::PSELECT6 if args[5] != 0 => {
            let mut bytes = [0; 16];
            process
                .address_space
                .read(args[5], &mut bytes)
                .map_err(|_| EFAULT)?;
            (word(&bytes, 0), word(&bytes, 8))
        }
        _ => return Ok(()),
    };
    if address == 0 {
        return Ok(());
    }
    if size != 8 {
        return Err(EINVAL);
    }
    let mut bytes = [0; 8];
    process
        .address_space
        .read(address, &mut bytes)
        .map_err(|_| EFAULT)?;
    let mut signals = process.threads[index].signals.borrow_mut();
    signals.suspended_mask = Some(signals.mask);
    signals.mask = u64::from_le_bytes(bytes) & !signal::UNBLOCKABLE;
    Ok(())
}
