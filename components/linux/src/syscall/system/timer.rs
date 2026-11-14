// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Linux process interval timer syscalls.

use crate::{
    process::Process,
    syscall::abi::{self, EFAULT, EINVAL, EIO, ENOMEM, Errno, SyscallResult},
};

const PROF: usize = 2;
const MAX_NS: u64 = i64::MAX as u64;

/// A validated `itimerval`.
#[derive(Clone, Copy, Default)]
struct Value {
    interval: u64,
    value: u64,
}

// REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/kernel/time/itimer.c?h=v6.12
pub(super) fn dispatch(
    process: &mut Process<'_>,
    number: usize,
    args: [usize; 6],
) -> SyscallResult {
    if number == abi::GETITIMER {
        get(process, args)
    } else {
        set(process, args)
    }
}

fn get(process: &Process<'_>, args: [usize; 6]) -> SyscallResult {
    let which = args[0];
    if which > PROF {
        return Err(EINVAL);
    }
    let now = runtime::thread::now_ns().map_err(error)?;
    let (remaining, interval) = process.timers.get(which, now, process.cpu_time_ns());
    write_value(process, args[1], remaining, interval)?;
    Ok(0)
}

fn set(process: &mut Process<'_>, args: [usize; 6]) -> SyscallResult {
    let which = args[0];
    if which > PROF {
        return Err(EINVAL);
    }
    let new = if args[1] == 0 {
        Value::default()
    } else {
        read_value(process, args[1])?
    };
    let now = runtime::thread::now_ns().map_err(error)?;
    let cpu = process.cpu_time_ns();
    let (remaining, interval) = process.timers.get(which, now, cpu);
    process
        .timers
        .set(which, new.value, new.interval, process.supervisor, cpu)
        .map_err(error)?;
    if args[2] != 0 {
        write_value(process, args[2], remaining, interval)?;
    }
    Ok(0)
}

fn read_value(process: &Process<'_>, address: usize) -> Result<Value, Errno> {
    let mut bytes = [0; 32];
    process
        .address_space
        .read(address, &mut bytes)
        .map_err(|_| EFAULT)?;
    Ok(Value {
        interval: duration(&bytes[..16])?,
        value: duration(&bytes[16..])?,
    })
}

fn duration(bytes: &[u8]) -> Result<u64, Errno> {
    let seconds = i64::from_le_bytes(bytes[..8].try_into().unwrap());
    let microseconds = i64::from_le_bytes(bytes[8..16].try_into().unwrap());
    if seconds < 0 || !(0..1_000_000).contains(&microseconds) {
        return Err(EINVAL);
    }
    Ok((seconds as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add((microseconds as u64).saturating_mul(1000))
        .min(MAX_NS))
}

fn write_value(
    process: &Process<'_>,
    address: usize,
    remaining: u64,
    interval: u64,
) -> Result<(), Errno> {
    let mut bytes = [0; 32];
    for (offset, duration) in [(0, interval), (16, remaining)] {
        bytes[offset..offset + 8].copy_from_slice(&(duration / 1_000_000_000).to_le_bytes());
        bytes[offset + 8..offset + 16]
            .copy_from_slice(&(duration % 1_000_000_000 / 1000).to_le_bytes());
    }
    process
        .address_space
        .write(address, &bytes)
        .map_err(|_| EFAULT)
}

fn error(error: runtime::Error) -> Errno {
    match error {
        runtime::Error::Kernel(runtime::KernelError::NoMemory) => ENOMEM,
        _ => EIO,
    }
}
