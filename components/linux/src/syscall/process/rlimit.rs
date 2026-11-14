// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Resource limit queries and updates.

use super::super::{
    Outcome,
    abi::{EFAULT, EINVAL, EPERM, ESRCH, SyscallResult},
};
use crate::{limits, limits::Rlimit, process::Process};

pub(super) fn start<'a>(process: &mut Process<'a>, arguments: [usize; 6]) -> Outcome<'a> {
    Outcome::Complete(change(process, arguments))
}

fn change(process: &mut Process<'_>, arguments: [usize; 6]) -> SyscallResult {
    // TODO: Address every visible process once the namespace exposes foreign limits.
    let pid = arguments[0] as i32;
    if pid != 0 && pid != process.pid as i32 {
        return Err(ESRCH);
    }
    let resource = arguments[1];
    let Some(previous) = process.limits.get(resource) else {
        return Err(EINVAL);
    };
    let requested = if arguments[2] == 0 {
        None
    } else {
        let mut bytes = [0; 16];
        process
            .address_space
            .read(arguments[2], &mut bytes)
            .map_err(|_| EFAULT)?;
        let value = Rlimit {
            current: u64::from_le_bytes(bytes[..8].try_into().unwrap()),
            maximum: u64::from_le_bytes(bytes[8..].try_into().unwrap()),
        };
        if value.current > value.maximum {
            return Err(EINVAL);
        }
        if value.maximum > previous.maximum && process.credentials.euid != 0 {
            return Err(EPERM);
        }
        Some(value)
    };
    if arguments[3] != 0 {
        let mut bytes = [0; 16];
        bytes[..8].copy_from_slice(&previous.current.to_le_bytes());
        bytes[8..].copy_from_slice(&previous.maximum.to_le_bytes());
        process
            .address_space
            .write(arguments[3], &bytes)
            .map_err(|_| EFAULT)?;
    }
    if let Some(value) = requested {
        process.limits.set(resource, value);
        if resource == limits::NOFILE {
            process.files.set_maximum(process.limits.file_descriptors());
        }
    }
    Ok(0)
}
