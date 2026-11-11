// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Monotonic timer descriptors and expiration counts.

use super::{Buffer, Pending, fd};
use crate::{
    file::{Descriptor, O_NONBLOCK, OpenFileDescription, TimerState},
    process::Process,
    syscall::{
        Operation, Outcome,
        abi::{EAGAIN, EBADF, EFAULT, EINVAL, EMFILE, EOPNOTSUPP, Errno, SyscallResult},
        wait,
    },
};
use alloc::rc::Rc;
use core::cell::Cell;
use runtime::{Rights, ipc::WaitItem};

// REFERENCE: https://man7.org/linux/man-pages/man2/timerfd_create.2.html
pub(in crate::syscall) fn create(
    process: &mut Process<'_>,
    clock: usize,
    flags: usize,
) -> SyscallResult {
    let flags = flags as u32;
    if flags & !(O_NONBLOCK | super::O_CLOEXEC) != 0 {
        return Err(EINVAL);
    }
    if clock as i32 != 1 {
        return Err(if matches!(clock as i32, 0 | 7 | 8 | 9) {
            EOPNOTSUPP
        } else {
            EINVAL
        });
    }
    let event = process
        .supervisor
        .event(Rights::WAIT | Rights::SIGNAL)
        .map_err(wait::error)?;
    let timer = process
        .supervisor
        .timer(&event, 1, Rights::WRITE)
        .map_err(wait::error)?;
    let state = TimerState {
        next: Cell::new(None),
        interval: Cell::new(0),
        event,
        timer,
    };
    process
        .files
        .insert(
            Descriptor::timerfd(state, flags & O_NONBLOCK, flags & super::O_CLOEXEC != 0),
            0,
        )
        .ok_or(EMFILE)
}

pub(in crate::syscall) fn settime(
    process: &Process<'_>,
    raw: usize,
    flags: usize,
    address: usize,
    old: usize,
) -> SyscallResult {
    let descriptor = process.files.get(fd(raw)?).ok_or(EBADF)?;
    let OpenFileDescription::Timerfd { state, .. } = descriptor.description.as_ref() else {
        return Err(EINVAL);
    };
    // CANCEL_ON_SET has no effect for a monotonic timer.
    if flags as u32 & !3 != 0 {
        return Err(EINVAL);
    }
    let mut bytes = [0; 32];
    process
        .address_space
        .read(address, &mut bytes)
        .map_err(|_| EFAULT)?;
    let interval = duration(&bytes[..16])?;
    let value = duration(&bytes[16..])?;
    let now = wait::now()?;
    let next = match value {
        0 => None,
        value if flags & 1 != 0 => Some(value),
        value => Some(now.checked_add(value).ok_or(EINVAL)?),
    };
    if old != 0 {
        write_timer(process, old, state, now)?;
    }
    match next {
        Some(next) => state.timer.arm(next),
        None => state.timer.cancel(),
    }
    .map_err(wait::error)?;
    state.interval.set(interval);
    state.next.set(next);
    Ok(0)
}

pub(in crate::syscall) fn gettime(
    process: &Process<'_>,
    raw: usize,
    address: usize,
) -> SyscallResult {
    let descriptor = process.files.get(fd(raw)?).ok_or(EBADF)?;
    let OpenFileDescription::Timerfd { state, .. } = descriptor.description.as_ref() else {
        return Err(EINVAL);
    };
    write_timer(process, address, state, wait::now()?)
}

fn write_timer(
    process: &Process<'_>,
    address: usize,
    state: &TimerState,
    now: u64,
) -> SyscallResult {
    let mut bytes = [0; 32];
    let remaining = future(state, now).map_or(0, |next| next.saturating_sub(now));
    for (offset, value) in [(0, state.interval.get()), (16, remaining)] {
        bytes[offset..offset + 8].copy_from_slice(&(value / 1_000_000_000).to_le_bytes());
        bytes[offset + 8..offset + 16].copy_from_slice(&(value % 1_000_000_000).to_le_bytes());
    }
    process
        .address_space
        .write(address, &bytes)
        .map(|()| 0)
        .map_err(|_| EFAULT)
}

fn duration(bytes: &[u8]) -> Result<u64, Errno> {
    let seconds = i64::from_le_bytes(bytes[..8].try_into().unwrap());
    let nanos = i64::from_le_bytes(bytes[8..].try_into().unwrap());
    if seconds < 0 || !(0..1_000_000_000).contains(&nanos) {
        return Err(EINVAL);
    }
    (seconds as u64)
        .checked_mul(1_000_000_000)
        .and_then(|value| value.checked_add(nanos as u64))
        .ok_or(EINVAL)
}

fn future(state: &TimerState, now: u64) -> Option<u64> {
    let next = state.next.get()?;
    if next > now {
        return Some(next);
    }
    let interval = state.interval.get();
    let count = (now - next).checked_div(interval)?.checked_add(1)?;
    next.checked_add(count.checked_mul(interval)?)
}

pub(in crate::syscall) fn read<'a>(
    process: &Process<'a>,
    raw: usize,
    buffer: Buffer,
) -> Outcome<'a> {
    let result = (|| {
        let descriptor = process.files.get(fd(raw)?).ok_or(EBADF)?;
        let buffers = buffer.import(process)?;
        if buffers.length() < 8 {
            return Err(EINVAL);
        }
        Ok(Outcome::Pending(Operation::File(Pending::Timerfd(Read {
            description: Rc::clone(&descriptor.description),
            buffers,
        }))))
    })();
    result.unwrap_or_else(|error| Outcome::Complete(Err(error)))
}

pub(in crate::syscall) struct Read<'a> {
    description: Rc<OpenFileDescription<'a>>,
    buffers: super::buffer::IoVectors,
}

impl Read<'_> {
    pub fn waits<'a>(&'a self, items: &mut alloc::vec::Vec<WaitItem<'a>>) {
        if let OpenFileDescription::Timerfd { state, .. } = self.description.as_ref() {
            items.push(WaitItem::new(&state.event, 1));
        }
    }

    pub fn poll(&mut self, process: &Process<'_>) -> Option<SyscallResult> {
        let OpenFileDescription::Timerfd { state, flags } = self.description.as_ref() else {
            unreachable!()
        };
        let now = match wait::now() {
            Ok(now) => now,
            Err(error) => return Some(Err(error)),
        };
        let Some(next) = state.next.get().filter(|next| *next <= now) else {
            return if flags.get() & O_NONBLOCK != 0 {
                Some(Err(EAGAIN))
            } else {
                None
            };
        };
        let count = (now - next)
            .checked_div(state.interval.get())
            .unwrap_or(0)
            .saturating_add(1);
        let mut bytes = count.to_le_bytes();
        match self.buffers.scatter(process, &mut bytes) {
            Ok(8) => {}
            Ok(_) => return Some(Err(EFAULT)),
            Err(error) => return Some(Err(error)),
        }
        let next = future(state, now);
        let armed = next.map_or_else(|| state.timer.cancel(), |next| state.timer.arm(next));
        if let Err(error) = armed {
            return Some(Err(wait::error(error)));
        }
        state.next.set(next);
        Some(Ok(8))
    }
}
