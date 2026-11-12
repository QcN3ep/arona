// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Linux scheduling policy, nice value, and CPU affinity calls.

use crate::{
    process::{Process, SCHED_BATCH, SCHED_IDLE, SCHED_OTHER},
    syscall::abi::{self, EFAULT, EINVAL, EIO, EPERM, ESRCH, Errno, SyscallResult},
};

const PRIO_PROCESS: i32 = 0;
const CPU_MASK_BYTES: usize = 8;
const NICE_BIAS: i32 = -runtime::thread::PRIORITY_MIN;

// REFERENCE: https://man7.org/linux/man-pages/man2/sched_setscheduler.2.html
pub(super) fn dispatch(
    process: &mut Process<'_>,
    index: usize,
    number: usize,
    args: [usize; 6],
) -> SyscallResult {
    match number {
        abi::SCHED_SETSCHEDULER => set_scheduler(process, index, args),
        abi::SCHED_GETSCHEDULER => {
            let target = sched_target(process, index, args[0])?;
            Ok(process.threads[target].policy as usize)
        }
        abi::SCHED_GETPARAM => get_param(process, index, args),
        abi::SCHED_SETPARAM => set_param(process, index, args),
        abi::SCHED_SETAFFINITY => set_affinity(process, index, args),
        abi::SCHED_GETAFFINITY => get_affinity(process, index, args),
        abi::SETPRIORITY => set_priority(process, index, args),
        abi::GETPRIORITY => get_priority(process, index, args),
        _ => unreachable!(),
    }
}

// REFERENCE: https://man7.org/linux/man-pages/man2/sched_setscheduler.2.html
fn set_scheduler(process: &mut Process<'_>, index: usize, args: [usize; 6]) -> SyscallResult {
    let target = sched_target(process, index, args[0])?;
    let policy = args[1] as i32;
    let requested = read_param(process, args[2])?;
    if requested != 0 {
        return Err(EINVAL);
    }
    if !matches!(policy, SCHED_OTHER | SCHED_BATCH | SCHED_IDLE) {
        // TODO: Support SCHED_FIFO and SCHED_RR once the kernel provides static priority bands.
        return Err(EINVAL);
    }
    // TODO: Give SCHED_BATCH and SCHED_IDLE distinct weights in the kernel scheduler.
    process.threads[target].policy = policy;
    Ok(0)
}

// REFERENCE: https://man7.org/linux/man-pages/man2/sched_getparam.2.html
fn get_param(process: &Process<'_>, index: usize, args: [usize; 6]) -> SyscallResult {
    sched_target(process, index, args[0])?;
    write_param(process, args[1], 0)?;
    Ok(0)
}

// REFERENCE: https://man7.org/linux/man-pages/man2/sched_setparam.2.html
fn set_param(process: &Process<'_>, index: usize, args: [usize; 6]) -> SyscallResult {
    sched_target(process, index, args[0])?;
    if read_param(process, args[1])? != 0 {
        // Non-real-time policies require a zero static priority.
        return Err(EINVAL);
    }
    Ok(0)
}

// REFERENCE: https://man7.org/linux/man-pages/man2/setpriority.2.html
fn set_priority(process: &Process<'_>, index: usize, args: [usize; 6]) -> SyscallResult {
    if args[0] as i32 != PRIO_PROCESS {
        return Err(EINVAL);
    }
    let target = target(process, index, args[1])?;
    // Linux clamps a nice value outside the supported range instead of failing.
    let nice = (args[2] as i64).clamp(
        i64::from(runtime::thread::PRIORITY_MIN),
        i64::from(runtime::thread::PRIORITY_MAX),
    );
    process.threads[target]
        .native
        .set_priority(nice as i32)
        .map_err(error)?;
    Ok(0)
}

// REFERENCE: https://man7.org/linux/man-pages/man2/getpriority.2.html
fn get_priority(process: &Process<'_>, index: usize, args: [usize; 6]) -> SyscallResult {
    if args[0] as i32 != PRIO_PROCESS {
        return Err(EINVAL);
    }
    let target = target(process, index, args[1])?;
    let nice = process.threads[target].native.priority().map_err(error)?;
    Ok((NICE_BIAS - nice) as usize)
}

// REFERENCE: https://man7.org/linux/man-pages/man2/sched_setaffinity.2.html
fn set_affinity(process: &Process<'_>, index: usize, args: [usize; 6]) -> SyscallResult {
    sched_target(process, index, args[0])?;
    if args[1] < CPU_MASK_BYTES || !args[1].is_multiple_of(CPU_MASK_BYTES) {
        return Err(EINVAL);
    }
    let mask = read_mask(process, args[2])?;
    // Arona currently has one processor, so CPU 0 is the only valid member.
    if mask != 1 {
        return Err(EINVAL);
    }
    Ok(0)
}

// REFERENCE: https://man7.org/linux/man-pages/man2/sched_getaffinity.2.html
fn get_affinity(process: &Process<'_>, index: usize, args: [usize; 6]) -> SyscallResult {
    sched_target(process, index, args[0])?;
    if args[1] < CPU_MASK_BYTES || !args[1].is_multiple_of(CPU_MASK_BYTES) {
        return Err(EINVAL);
    }
    process
        .address_space
        .write(args[2], &1u64.to_le_bytes())
        .map_err(|_| EFAULT)?;
    // The raw syscall reports the number of bytes written; only the C library
    // wrapper turns this into zero.
    Ok(CPU_MASK_BYTES)
}

/// Resolves a scheduler call target; Linux rejects negative thread IDs here.
fn sched_target(process: &Process<'_>, index: usize, raw: usize) -> Result<usize, Errno> {
    if (raw as i32) < 0 {
        return Err(EINVAL);
    }
    target(process, index, raw)
}

fn target(process: &Process<'_>, index: usize, raw: usize) -> Result<usize, Errno> {
    let tid = raw as i32;
    if tid == 0 {
        return Ok(index);
    }
    if tid < 0 {
        return Err(ESRCH);
    }
    process
        .threads
        .iter()
        .position(|thread| thread.tid == tid as usize)
        .ok_or(ESRCH)
}

fn read_param(process: &Process<'_>, address: usize) -> Result<i32, Errno> {
    let mut bytes = [0; 4];
    process
        .address_space
        .read(address, &mut bytes)
        .map_err(|_| EFAULT)?;
    Ok(i32::from_le_bytes(bytes))
}

fn write_param(process: &Process<'_>, address: usize, value: i32) -> Result<(), Errno> {
    process
        .address_space
        .write(address, &value.to_le_bytes())
        .map_err(|_| EFAULT)
}

fn read_mask(process: &Process<'_>, address: usize) -> Result<u64, Errno> {
    let mut bytes = [0; CPU_MASK_BYTES];
    process
        .address_space
        .read(address, &mut bytes)
        .map_err(|_| EFAULT)?;
    Ok(u64::from_le_bytes(bytes))
}

fn error(error: runtime::Error) -> Errno {
    match error {
        runtime::Error::Kernel(runtime::KernelError::InvalidArgument) => EINVAL,
        runtime::Error::Kernel(runtime::KernelError::Denied) => EPERM,
        _ => EIO,
    }
}
