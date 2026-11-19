// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Kernel log control and retrieval.

use crate::{
    Services,
    process::Process,
    syscall::{
        Outcome,
        abi::{EFAULT, EINVAL, EPERM, SyscallResult},
    },
};

// REFERENCE: https://man7.org/linux/man-pages/man2/syslog.2.html
const ACTION_CLOSE: i32 = 0;
const ACTION_OPEN: i32 = 1;
const ACTION_READ: i32 = 2;
const ACTION_READ_ALL: i32 = 3;
const ACTION_READ_CLEAR: i32 = 4;
const ACTION_CLEAR: i32 = 5;
const ACTION_CONSOLE_OFF: i32 = 6;
const ACTION_CONSOLE_ON: i32 = 7;
const ACTION_CONSOLE_LEVEL: i32 = 8;
const ACTION_SIZE_UNREAD: i32 = 9;
const ACTION_SIZE_BUFFER: i32 = 10;

pub(super) fn dispatch<'a>(
    process: &Process<'a>,
    services: &Services<'_>,
    args: [usize; 6],
) -> Option<Outcome<'a>> {
    Some(Outcome::Complete(call(process, services, args)))
}

fn call(process: &Process<'_>, services: &Services<'_>, args: [usize; 6]) -> SyscallResult {
    let action = args[0] as i32;
    if !(ACTION_CLOSE..=ACTION_SIZE_BUFFER).contains(&action) {
        return Err(EINVAL);
    }
    // Only OPEN and CLOSE bypass the CAP_SYSLOG check. The compatibility layer
    // has no capability sets, so effective uid zero stands in for CAP_SYSLOG.
    if !matches!(action, ACTION_OPEN | ACTION_CLOSE) && process.credentials.euid != 0 {
        return Err(EPERM);
    }
    match action {
        ACTION_CLOSE | ACTION_OPEN => Ok(0),
        // Console log levels affect physical console output, which this layer
        // does not control. Report them as unsupported instead of succeeding.
        ACTION_CONSOLE_OFF | ACTION_CONSOLE_ON | ACTION_CONSOLE_LEVEL => Err(EINVAL),
        ACTION_SIZE_UNREAD => services.log.size_unread(),
        ACTION_SIZE_BUFFER => services.log.capacity(),
        ACTION_CLEAR => services.log.clear().map(|()| 0),
        ACTION_READ | ACTION_READ_ALL | ACTION_READ_CLEAR => {
            let length = args[2] as i32;
            if length < 0 {
                return Err(EINVAL);
            }
            let length = length as usize;
            if length == 0 {
                return Ok(0);
            }
            if args[1] == 0 {
                return Err(EFAULT);
            }
            match action {
                ACTION_READ => {
                    let (bytes, end) = services.log.read(length)?;
                    process
                        .address_space
                        .write(args[1], &bytes)
                        .map_err(|_| EFAULT)?;
                    services.log.commit_read(end);
                    Ok(bytes.len())
                }
                ACTION_READ_ALL => {
                    let bytes = services.log.read_all(length)?;
                    process
                        .address_space
                        .write(args[1], &bytes)
                        .map_err(|_| EFAULT)?;
                    Ok(bytes.len())
                }
                _ => {
                    let (bytes, next) = services.log.read_clear(length)?;
                    process
                        .address_space
                        .write(args[1], &bytes)
                        .map_err(|_| EFAULT)?;
                    services.log.commit_clear(next);
                    Ok(bytes.len())
                }
            }
        }
        _ => Err(EINVAL),
    }
}
