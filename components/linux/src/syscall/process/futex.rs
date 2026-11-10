// SPDX-License-Identifier: MPL-2.0

//! Linux 非 PI futex 等待与唤醒。

use crate::{
    process::{Process, futex::Registration},
    syscall::{
        Operation, Outcome,
        abi::{EAGAIN, EFAULT, EINVAL, EOPNOTSUPP, ETIMEDOUT, Errno, SyscallResult},
        wait::{self, Deadline},
    },
};
use runtime::ipc::WaitItem;

pub(in crate::syscall) struct Pending {
    registration: Registration,
    deadline: Deadline,
}

// REFERENCE: https://man7.org/linux/man-pages/man2/futex.2.html
pub(super) fn start<'a>(process: &Process<'a>, args: [usize; 6]) -> Outcome<'a> {
    let result = (|| {
        let [address, operation, value, timeout, second, bits] = args;
        let operation = operation as u32 as usize;
        let value = value as u32 as usize;
        let private = operation & 128 != 0;
        let command = operation & 127;
        let realtime = operation & 256 != 0;
        if operation & !511 != 0 || realtime && command != 9 {
            return Err(EOPNOTSUPP);
        }
        if realtime {
            // TODO: 接入 CLOCK_REALTIME 的 futex 绝对期限。
            return Err(EOPNOTSUPP);
        }
        let (key, frames) = process
            .address_space
            .futex_key(process.pid, address, private)?;
        match command {
            0 | 9 => {
                let mask = if command == 9 { bits as u32 } else { u32::MAX };
                if mask == 0 {
                    return Err(EINVAL);
                }
                if read(process, address)? != value as u32 {
                    return Err(EAGAIN);
                }
                let mut duration = wait::duration(process, timeout)?;
                if command == 9 {
                    duration = {
                        let now = wait::now()?;
                        duration.map(|absolute| absolute.saturating_sub(now))
                    };
                }
                let deadline = Deadline::new(process.supervisor, duration)?;
                let registration = process.namespace.futexes.register(key, mask, frames);
                Ok(Outcome::Pending(Operation::Futex(Pending {
                    registration,
                    deadline,
                })))
            }
            1 | 10 => {
                let mask = if command == 10 { bits as u32 } else { u32::MAX };
                if mask == 0 || (value as i32) < 0 {
                    return Err(EINVAL);
                }
                Ok(Outcome::Complete(Ok(process
                    .namespace
                    .futexes
                    .wake(key, value, mask))))
            }
            3 | 4 => {
                let timeout = timeout as u32 as usize;
                if (value as i32) < 0 || (timeout as i32) < 0 {
                    return Err(EINVAL);
                }
                if command == 4 && read(process, address)? != bits as u32 {
                    return Err(EAGAIN);
                }
                let (target, target_frames) =
                    process
                        .address_space
                        .futex_key(process.pid, second, private)?;
                if key == target {
                    return Err(EINVAL);
                }
                let count = process.namespace.futexes.wake(key, value, u32::MAX);
                Ok(Outcome::Complete(Ok(count
                    + process.namespace.futexes.requeue(
                        key,
                        target,
                        target_frames,
                        timeout,
                    ))))
            }
            _ => Err(EOPNOTSUPP),
        }
    })();
    result.unwrap_or_else(|error| Outcome::Complete(Err(error)))
}

fn read(process: &Process<'_>, address: usize) -> Result<u32, Errno> {
    let mut bytes = [0; 4];
    process
        .address_space
        .read(address, &mut bytes)
        .map_err(|_| EFAULT)?;
    Ok(u32::from_le_bytes(bytes))
}

impl Pending {
    pub fn restartable(&self) -> bool {
        // TODO: 通过 restart_syscall 保留有期限 futex 等待的绝对截止时间。
        self.deadline
            .remaining()
            .is_ok_and(|remaining| remaining.is_none())
    }

    pub fn waits<'a>(&'a self, items: &mut alloc::vec::Vec<WaitItem<'a>>) {
        self.deadline.waits(items);
    }
    pub fn poll(&self) -> Option<SyscallResult> {
        if self.registration.ready() {
            return Some(Ok(0));
        }
        match self.deadline.expired() {
            Ok(true) => Some(Err(ETIMEDOUT)),
            Ok(false) => None,
            Err(error) => Some(Err(error)),
        }
    }
}
