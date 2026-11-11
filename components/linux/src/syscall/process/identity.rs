// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Linux credentials, identity queries, sessions, and process groups.

// TODO: Enforce credentials in filesystem operations.

use crate::{
    process::{Credentials, Process},
    syscall::abi::{self, EFAULT, EINVAL, EPERM, SyscallResult},
};

const NO_CHANGE: u32 = u32::MAX;

pub(super) fn dispatch(
    process: &mut Process<'_>,
    number: usize,
    args: [usize; 6],
) -> Option<SyscallResult> {
    Some(match number {
        abi::GETUID => Ok(process.credentials.uid as usize),
        abi::GETEUID => Ok(process.credentials.euid as usize),
        abi::GETGID => Ok(process.credentials.gid as usize),
        abi::GETEGID => Ok(process.credentials.egid as usize),
        abi::GETRESUID => write_ids(
            process,
            &args[..3],
            [
                process.credentials.uid,
                process.credentials.euid,
                process.credentials.suid,
            ],
        ),
        abi::GETRESGID => write_ids(
            process,
            &args[..3],
            [
                process.credentials.gid,
                process.credentials.egid,
                process.credentials.sgid,
            ],
        ),
        abi::SETUID => set_user(process, args[0]),
        abi::SETGID => set_group(process, args[0]),
        abi::SETREUID => set_reuid(process, args[0], args[1]),
        abi::SETREGID => set_regid(process, args[0], args[1]),
        abi::SETRESUID => set_resuid(process, args[0], args[1], args[2]),
        abi::SETRESGID => set_resgid(process, args[0], args[1], args[2]),
        abi::GETGROUPS => {
            if (args[0] as i32) < 0 {
                Err(EINVAL)
            } else {
                Ok(0)
            }
        }
        abi::SETGROUPS => {
            if (args[0] as i32) < 0 {
                Err(EINVAL)
            } else if process.credentials.euid != 0 || args[0] != 0 {
                Err(EPERM)
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

fn write_ids(process: &Process<'_>, addresses: &[usize], ids: [u32; 3]) -> SyscallResult {
    for (address, value) in addresses.iter().zip(ids) {
        process
            .address_space
            .write(*address, &value.to_le_bytes())
            .map_err(|_| EFAULT)?;
    }
    Ok(0)
}

fn privileged(credentials: &Credentials) -> bool {
    credentials.euid == 0
}

fn set_user(process: &mut Process<'_>, raw: usize) -> SyscallResult {
    let value = raw as u32;
    if value == NO_CHANGE {
        return Err(EINVAL);
    }
    let credentials = &mut process.credentials;
    if privileged(credentials) {
        credentials.uid = value;
        credentials.euid = value;
        credentials.suid = value;
        Ok(0)
    } else if value == credentials.uid || value == credentials.suid {
        credentials.euid = value;
        Ok(0)
    } else {
        Err(EPERM)
    }
}

fn set_group(process: &mut Process<'_>, raw: usize) -> SyscallResult {
    let value = raw as u32;
    if value == NO_CHANGE {
        return Err(EINVAL);
    }
    let credentials = &mut process.credentials;
    if privileged(credentials) {
        credentials.gid = value;
        credentials.egid = value;
        credentials.sgid = value;
        Ok(0)
    } else if value == credentials.gid || value == credentials.sgid {
        credentials.egid = value;
        Ok(0)
    } else {
        Err(EPERM)
    }
}

// REFERENCE: https://man7.org/linux/man-pages/man2/setreuid.2.html
fn set_reuid(process: &mut Process<'_>, real: usize, effective: usize) -> SyscallResult {
    let real = real as u32;
    let effective = effective as u32;
    let credentials = &mut process.credentials;
    if !privileged(credentials)
        && ((real != NO_CHANGE && real != credentials.uid && real != credentials.euid)
            || (effective != NO_CHANGE
                && ![credentials.uid, credentials.euid, credentials.suid].contains(&effective)))
    {
        return Err(EPERM);
    }
    let save = real != NO_CHANGE || (effective != NO_CHANGE && effective != credentials.uid);
    if real != NO_CHANGE {
        credentials.uid = real;
    }
    if effective != NO_CHANGE {
        credentials.euid = effective;
    }
    if save {
        credentials.suid = credentials.euid;
    }
    Ok(0)
}

fn set_regid(process: &mut Process<'_>, real: usize, effective: usize) -> SyscallResult {
    let real = real as u32;
    let effective = effective as u32;
    let credentials = &mut process.credentials;
    if !privileged(credentials)
        && ((real != NO_CHANGE && real != credentials.gid && real != credentials.egid)
            || (effective != NO_CHANGE
                && ![credentials.gid, credentials.egid, credentials.sgid].contains(&effective)))
    {
        return Err(EPERM);
    }
    let save = real != NO_CHANGE || (effective != NO_CHANGE && effective != credentials.gid);
    if real != NO_CHANGE {
        credentials.gid = real;
    }
    if effective != NO_CHANGE {
        credentials.egid = effective;
    }
    if save {
        credentials.sgid = credentials.egid;
    }
    Ok(0)
}

fn set_resuid(
    process: &mut Process<'_>,
    real: usize,
    effective: usize,
    saved: usize,
) -> SyscallResult {
    let values = [real as u32, effective as u32, saved as u32];
    let credentials = &mut process.credentials;
    if !privileged(credentials)
        && !values.iter().all(|value| {
            *value == NO_CHANGE
                || *value == credentials.uid
                || *value == credentials.euid
                || *value == credentials.suid
        })
    {
        return Err(EPERM);
    }
    if values[0] != NO_CHANGE {
        credentials.uid = values[0];
    }
    if values[1] != NO_CHANGE {
        credentials.euid = values[1];
    }
    if values[2] != NO_CHANGE {
        credentials.suid = values[2];
    }
    Ok(0)
}

fn set_resgid(
    process: &mut Process<'_>,
    real: usize,
    effective: usize,
    saved: usize,
) -> SyscallResult {
    let values = [real as u32, effective as u32, saved as u32];
    let credentials = &mut process.credentials;
    if !privileged(credentials)
        && !values.iter().all(|value| {
            *value == NO_CHANGE
                || *value == credentials.gid
                || *value == credentials.egid
                || *value == credentials.sgid
        })
    {
        return Err(EPERM);
    }
    if values[0] != NO_CHANGE {
        credentials.gid = values[0];
    }
    if values[1] != NO_CHANGE {
        credentials.egid = values[1];
    }
    if values[2] != NO_CHANGE {
        credentials.sgid = values[2];
    }
    Ok(0)
}
