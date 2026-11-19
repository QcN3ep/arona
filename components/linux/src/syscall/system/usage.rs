// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! `sysinfo`, `times`, and `getrusage`.

use crate::{
    process::Process,
    syscall::{
        abi::{EFAULT, EIO, SyscallResult},
        rusage,
    },
};

/// Linux `USER_HZ`; `times` and `/proc/<pid>/stat` report clock ticks at this rate.
pub(super) const TICKS_PER_SECOND: u64 = 100;
const NANOS_PER_TICK: u64 = 1_000_000_000 / TICKS_PER_SECOND;

// REFERENCE: https://man7.org/linux/man-pages/man2/sysinfo.2.html
/// Size of `struct sysinfo` on 64-bit Linux, including the trailing alignment.
const SYSINFO_SIZE: usize = 112;

// REFERENCE: https://man7.org/linux/man-pages/man2/times.2.html
const TMS_SIZE: usize = 4 * 8;

pub(super) fn sysinfo(
    process: &Process<'_>,
    services: &crate::Services<'_>,
    address: usize,
) -> SyscallResult {
    let now = runtime::thread::now_ns().map_err(|_| EIO)?;
    let (total, free) = services
        .system
        .as_ref()
        .ok_or(EIO)?
        .memory_stats()
        .map_err(|_| EIO)?;
    let loads = services.loads.get();
    let processes = process.namespace.processes.borrow().len();
    let mut bytes = [0; SYSINFO_SIZE];
    bytes[..8].copy_from_slice(&((now / 1_000_000_000) as i64).to_le_bytes());
    for (index, load) in loads.into_iter().enumerate() {
        let offset = 8 + index * 8;
        bytes[offset..offset + 8].copy_from_slice(&u64::from(load).to_le_bytes());
    }
    // No swap or page cache is reported, so only total and free memory are set.
    for (offset, value) in [(32, total), (40, free)] {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    let processes = processes.min(usize::from(u16::MAX)) as u16;
    bytes[80..82].copy_from_slice(&processes.to_le_bytes());
    // Memory is reported in bytes: the unit for every field is mem_unit bytes.
    bytes[104..108].copy_from_slice(&1u32.to_le_bytes());
    process
        .address_space
        .write(address, &bytes)
        .map_err(|_| EFAULT)?;
    Ok(0)
}

pub(super) fn times(process: &Process<'_>, address: usize) -> SyscallResult {
    let now = runtime::thread::now_ns().map_err(|_| EIO)?;
    let children = process
        .namespace
        .processes
        .borrow()
        .get(&process.pid)
        .map_or(0, |record| record.children.cpu_ns);
    let fields = [ticks(process.cpu_ns()), 0, ticks(children), 0];
    let mut bytes = [0; TMS_SIZE];
    for (chunk, value) in bytes.as_chunks_mut::<8>().0.iter_mut().zip(fields) {
        chunk.copy_from_slice(&(value as i64).to_le_bytes());
    }
    process
        .address_space
        .write(address, &bytes)
        .map_err(|_| EFAULT)?;
    Ok(ticks(now) as usize)
}

pub(super) fn getrusage(process: &Process<'_>, who: usize, address: usize) -> SyscallResult {
    let usage = rusage::process_usage(process, who as i32)?;
    let bytes = rusage::encode(usage);
    process
        .address_space
        .write(address, &bytes)
        .map_err(|_| EFAULT)?;
    Ok(0)
}

fn ticks(nanoseconds: u64) -> u64 {
    nanoseconds / NANOS_PER_TICK
}
