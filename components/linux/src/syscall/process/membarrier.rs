// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! The `membarrier` process-wide memory barrier call.

use core::sync::atomic::{Ordering, compiler_fence, fence};

use crate::{
    process::Process,
    syscall::abi::{EINVAL, EOPNOTSUPP, EPERM, SyscallResult},
};

const CMD_GLOBAL: usize = 1 << 0;
const CMD_GLOBAL_EXPEDITED: usize = 1 << 1;
const CMD_REGISTER_GLOBAL_EXPEDITED: usize = 1 << 2;
const CMD_PRIVATE_EXPEDITED: usize = 1 << 3;
const CMD_REGISTER_PRIVATE_EXPEDITED: usize = 1 << 4;
const CMD_PRIVATE_EXPEDITED_SYNC_CORE: usize = 1 << 5;
const CMD_REGISTER_PRIVATE_EXPEDITED_SYNC_CORE: usize = 1 << 6;
const CMD_PRIVATE_EXPEDITED_RSEQ: usize = 1 << 7;
const CMD_REGISTER_PRIVATE_EXPEDITED_RSEQ: usize = 1 << 8;
const CMD_GET_REGISTRATIONS: usize = 1 << 9;

const SUPPORTED: usize = CMD_GLOBAL
    | CMD_GLOBAL_EXPEDITED
    | CMD_REGISTER_GLOBAL_EXPEDITED
    | CMD_PRIVATE_EXPEDITED
    | CMD_REGISTER_PRIVATE_EXPEDITED
    | CMD_GET_REGISTRATIONS;

const KNOWN_UNSUPPORTED: usize = CMD_PRIVATE_EXPEDITED_SYNC_CORE
    | CMD_REGISTER_PRIVATE_EXPEDITED_SYNC_CORE
    | CMD_PRIVATE_EXPEDITED_RSEQ
    | CMD_REGISTER_PRIVATE_EXPEDITED_RSEQ;

// REFERENCE: https://man7.org/linux/man-pages/man2/membarrier.2.html
pub(super) fn dispatch(process: &mut Process<'_>, args: [usize; 6]) -> SyscallResult {
    let command = args[0];
    // The flag that selects one CPU is not meaningful on a uniprocessor.
    if args[1] != 0 {
        return Err(EINVAL);
    }
    match command {
        0 => Ok(SUPPORTED),
        CMD_GLOBAL | CMD_GLOBAL_EXPEDITED => {
            barrier();
            Ok(0)
        }
        CMD_REGISTER_GLOBAL_EXPEDITED => {
            process.membarrier.global_expedited = true;
            Ok(0)
        }
        CMD_REGISTER_PRIVATE_EXPEDITED => {
            process.membarrier.private_expedited = true;
            Ok(0)
        }
        CMD_PRIVATE_EXPEDITED if process.membarrier.private_expedited => {
            barrier();
            Ok(0)
        }
        CMD_PRIVATE_EXPEDITED => Err(EPERM),
        CMD_GET_REGISTRATIONS => Ok(registrations(process)),
        _ if command & KNOWN_UNSUPPORTED != 0 => Err(EOPNOTSUPP),
        _ => Err(EINVAL),
    }
}

/// Orders previous memory accesses before later ones for every thread on this CPU.
///
/// All user threads share one processor, so there is no remote CPU to notify.
/// A sequentially consistent fence and compiler barrier provide the ordering
/// that per-CPU barriers would otherwise require.
fn barrier() {
    compiler_fence(Ordering::SeqCst);
    fence(Ordering::SeqCst);
    compiler_fence(Ordering::SeqCst);
}

fn registrations(process: &Process<'_>) -> usize {
    let mut mask = 0;
    if process.membarrier.global_expedited {
        mask |= CMD_REGISTER_GLOBAL_EXPEDITED;
    }
    if process.membarrier.private_expedited {
        mask |= CMD_REGISTER_PRIVATE_EXPEDITED;
    }
    mask
}
