// SPDX-License-Identifier: MPL-2.0

//! Linux identity queries, sessions, and process group operations.

use crate::{
    process::Process,
    syscall::abi::{self, EFAULT, EINVAL, SyscallResult},
};

pub(super) fn dispatch(
    process: &Process<'_>,
    number: usize,
    args: [usize; 6],
) -> Option<SyscallResult> {
    Some(match number {
        // TODO: Support Linux credential changes, inheritance, and access permission checks.
        abi::GETUID | abi::GETEUID | abi::GETGID | abi::GETEGID => Ok(0),
        abi::GETRESUID | abi::GETRESGID => credentials(process, args),
        abi::GETGROUPS => {
            if (args[0] as i32) < 0 {
                Err(EINVAL)
            } else {
                Ok(0)
            }
        }
        abi::GETPGID | abi::GETSID => {
            process
                .namespace
                .identity(pid(process, args[0]))
                .map(|(group, session)| {
                    if number == abi::GETPGID {
                        group
                    } else {
                        session
                    }
                })
        }
        abi::SETPGID => {
            let pid = pid(process, args[0]);
            let group = if args[1] as i32 == 0 {
                pid as i32
            } else {
                args[1] as i32
            };
            if group < 0 {
                Err(EINVAL)
            } else {
                process
                    .namespace
                    .set_group(process.pid, pid, group as usize)
                    .map(|()| 0)
            }
        }
        abi::SETSID => process.namespace.new_session(process.pid),
        _ => return None,
    })
}

fn pid(process: &Process<'_>, raw: usize) -> usize {
    let pid = raw as i32;
    if pid == 0 { process.pid } else { pid as usize }
}

fn credentials(process: &Process<'_>, args: [usize; 6]) -> SyscallResult {
    for address in &args[..3] {
        process
            .address_space
            .write(*address, &[0; 4])
            .map_err(|_| EFAULT)?;
    }
    Ok(0)
}
