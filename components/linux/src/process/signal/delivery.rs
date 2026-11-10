// SPDX-License-Identifier: MPL-2.0

//! 信号与用户陷入续体的交接。

use super::{Action, Info, KILL, NODEFER, RESETHAND, STOP, bit, frame, ignored};
use crate::{
    Error, ExitStatus,
    process::{Process, namespace::ChildChange},
    syscall::Context,
};

pub fn next(process: &Process<'_>, index: usize) -> Option<(Info, Action)> {
    loop {
        let thread = process.threads[index].signals.borrow();
        let signals = process.signals.borrow();
        let mask = if signals.stopped {
            bit(KILL)
        } else {
            !thread.mask
        };
        let pending = thread.pending.mask() | signals.pending.mask();
        let mask = if pending & bit(KILL) != 0 {
            bit(KILL)
        } else {
            mask
        };
        let info = thread
            .pending
            .first(mask)
            .or_else(|| signals.pending.first(mask))?;
        let action = signals.actions[usize::from(info.number) - 1];
        drop(signals);
        drop(thread);
        if ignored(info.number, action) {
            take(process, index, info.number);
        } else {
            return Some((info, action));
        }
    }
}

fn take(process: &Process<'_>, index: usize, number: u8) {
    if process.threads[index]
        .signals
        .borrow_mut()
        .pending
        .take(bit(number))
        .is_none()
    {
        process.signals.borrow_mut().pending.take(bit(number));
    }
}

pub fn terminate(process: &Process<'_>, signal: u8) -> Result<(), Error> {
    for thread in &process.threads {
        thread
            .native
            .terminate(ExitStatus { code: 0, signal }.encode())?;
    }
    Ok(())
}

fn stop(process: &Process<'_>, signal: u8) {
    if !process.signals.borrow().stopped {
        process.signals.borrow_mut().stopped = true;
        process
            .namespace
            .change(process.pid, ChildChange::Stopped(signal));
    }
}

pub fn poll(process: &mut Process<'_>, index: usize) -> Result<bool, Error> {
    let selected = next(process, index);
    if let Some((info, action)) = selected {
        if info.number == KILL || action.handler == 0 && !matches!(info.number, STOP | 20 | 21 | 22)
        {
            take(process, index, info.number);
            terminate(process, info.number)?;
            return Ok(true);
        }
        if action.handler == 0 && matches!(info.number, STOP | 20 | 21 | 22) {
            take(process, index, info.number);
            stop(process, info.number);
        }
    }
    let stopped = process.signals.borrow().stopped;
    if !stopped && let Some(context) = process.threads[index].held.take() {
        resume(process, index, context)?;
        return Ok(true);
    }
    let thread = &mut process.threads[index];
    if (stopped || selected.is_some())
        && thread.pending.is_none()
        && thread.held.is_none()
        && !thread.interrupt_requested
    {
        match thread.native.interrupt() {
            Ok(()) => {
                thread.interrupt_requested = true;
                return Ok(true);
            }
            Err(runtime::Error::Kernel(runtime::KernelError::InvalidState)) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(false)
}

pub fn resume(process: &mut Process<'_>, index: usize, mut context: Context) -> Result<(), Error> {
    if process.signals.borrow().stopped {
        process.threads[index].held = Some(context);
        return Ok(());
    }
    if let Some((info, action)) = next(process, index) {
        take(process, index, info.number);
        if action.handler == 0 {
            if matches!(info.number, STOP | 20 | 21 | 22) {
                stop(process, info.number);
                process.threads[index].held = Some(context);
                return Ok(());
            }
            return terminate(process, info.number);
        }
        if install(process, index, &mut context, info, action).is_err() {
            return terminate(process, 11);
        }
    } else {
        restore_mask(process, index);
    }
    match context.trap.resume(&context.state) {
        Ok(()) => Ok(()),
        Err((
            runtime::Error::Kernel(
                runtime::KernelError::InvalidArgument | runtime::KernelError::BadAddress,
            ),
            _,
        )) => terminate(process, 11),
        Err((error, _)) => Err(error.into()),
    }
}

pub fn restore_mask(process: &Process<'_>, index: usize) {
    let mut signals = process.threads[index].signals.borrow_mut();
    if let Some(mask) = signals.suspended_mask.take() {
        signals.mask = mask;
    }
}

fn install(
    process: &mut Process<'_>,
    index: usize,
    context: &mut Context,
    info: Info,
    action: Action,
) -> Result<(), runtime::Error> {
    let trampoline = if let Some(address) = process.signal_return {
        address
    } else {
        let address = process
            .address_space
            .map_code(&frame::trampoline(process.machine))?;
        process.signal_return = Some(address);
        address
    };
    let mut thread = process.threads[index].signals.borrow_mut();
    let mask = thread.suspended_mask.take().unwrap_or(thread.mask);
    frame::install(
        &process.address_space,
        process.machine,
        &mut context.state,
        frame::Delivery {
            info,
            action,
            mask,
            altstack: thread.altstack,
            trampoline,
        },
    )
    .map_err(|_| runtime::Error::Kernel(runtime::KernelError::BadAddress))?;
    thread.mask |= action.mask
        | if action.flags & NODEFER == 0 {
            bit(info.number)
        } else {
            0
        };
    if action.flags & RESETHAND != 0 {
        process.signals.borrow_mut().actions[usize::from(info.number) - 1] = Action::default();
    }
    Ok(())
}

pub fn fault(process: &mut Process<'_>, index: usize, mut context: Context) -> Result<(), Error> {
    let info = fault_info(&process.address_space, &context.trap.info);
    let action = process.signals.borrow().actions[usize::from(info.number) - 1];
    if action.handler <= 1 || process.threads[index].signals.borrow().mask & bit(info.number) != 0 {
        return terminate(process, info.number);
    }
    if install(process, index, &mut context, info, action).is_err() {
        return terminate(process, 11);
    }
    resume(process, index, context)
}

fn fault_info(space: &crate::memory::AddressSpace, trap: &runtime::trap::TrapInfo) -> Info {
    use runtime::trap::TrapInfo;
    let address = trap.address as usize;
    let (number, code, address) = match trap.kind {
        TrapInfo::ILLEGAL_INSTRUCTION => (4, 1, trap.pc as usize),
        TrapInfo::PAGE_FAULT if space.bus_fault(address, trap.detail) => (7, 2, address),
        TrapInfo::PAGE_FAULT => {
            let mapped = space
                .native
                .task()
                .query(address)
                .ok()
                .flatten()
                .is_some_and(|info| info.permissions.is_some());
            (11, if mapped { 2 } else { 1 }, address)
        }
        // TODO: 补齐其他异常对应的 Linux 信号。
        _ => (11, 1, address),
    };
    Info {
        number,
        code,
        address,
        sender: 0,
        value: 0,
    }
}
