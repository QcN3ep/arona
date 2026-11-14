// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Shared `rusage` accounting for `wait4` and `getrusage`.
//!
//! Guest threads run user code and block while the Linux service performs their
//! system calls, so the measured CPU time is reported as user time. Serviced page
//! faults and context switches are not accounted by the kernel: every mapping is
//! populated eagerly, and no switch counters are exposed, so those fields stay 0.

use crate::{
    process::{Process, namespace::Usage},
    syscall::abi::{EINVAL, Errno},
};

pub(super) const RUSAGE_SELF: i32 = 0;
pub(super) const RUSAGE_CHILDREN: i32 = -1;

/// Size of `struct rusage` on 64-bit Linux.
pub(super) const RUSAGE_SIZE: usize = 144;

pub(super) fn process_usage(process: &Process<'_>, who: i32) -> Result<Usage, Errno> {
    match who {
        RUSAGE_SELF => Ok(Usage {
            cpu_ns: process.cpu_ns(),
            min_flt: 0,
            maj_flt: 0,
            max_rss: process.max_rss.max(process.resident_bytes()),
        }),
        RUSAGE_CHILDREN => Ok(process
            .namespace
            .processes
            .borrow()
            .get(&process.pid)
            .map_or(Usage::default(), |record| record.children)),
        // TODO: Report per-thread accounting for RUSAGE_THREAD.
        _ => Err(EINVAL),
    }
}

pub(super) fn with_children(usage: Usage, children: Usage) -> Usage {
    Usage {
        cpu_ns: usage.cpu_ns.saturating_add(children.cpu_ns),
        min_flt: usage.min_flt.saturating_add(children.min_flt),
        maj_flt: usage.maj_flt.saturating_add(children.maj_flt),
        max_rss: usage.max_rss.max(children.max_rss),
    }
}

/// Encode `struct rusage` with timeval user and system time followed by peak memory
/// (KiB), faults, block I/O, signals, and switch counts.
pub(super) fn encode(usage: Usage) -> [u8; RUSAGE_SIZE] {
    let mut bytes = [0; RUSAGE_SIZE];
    let (seconds, micros) = split_time(usage.cpu_ns);
    bytes[..8].copy_from_slice(&seconds.to_le_bytes());
    bytes[8..16].copy_from_slice(&micros.to_le_bytes());
    let max_rss = usage.max_rss / 1024;
    for (offset, value) in [
        (32, max_rss),
        (40, 0),
        (48, 0),
        (56, 0),
        (64, usage.min_flt),
        (72, usage.maj_flt),
        (80, 0),
        (88, 0),
        (96, 0),
        (104, 0),
        (112, 0),
        (120, 0),
        (128, 0),
        (136, 0),
    ] {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    bytes
}

pub(super) fn split_time(nanoseconds: u64) -> (i64, i64) {
    (
        (nanoseconds / 1_000_000_000) as i64,
        ((nanoseconds % 1_000_000_000) / 1000) as i64,
    )
}
