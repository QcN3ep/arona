// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Linux non-PI futex waiting and wakeup.

// TODO: Complete the remaining futex operations.

use crate::{
    Services,
    process::{Process, futex::Registration},
    syscall::{
        Operation, Outcome,
        abi::{EAGAIN, EFAULT, EINVAL, EIO, EOPNOTSUPP, ETIMEDOUT, Errno, SyscallResult},
        wait::{self, Deadline},
    },
};
use runtime::{KernelError, ipc::WaitItem};

pub(in crate::syscall) struct Pending {
    registration: Registration,
    deadline: Deadline,
    clock: Option<devmgr::device::Pending<devmgr::rtc::Timestamp>>,
    realtime: Option<u64>,
}

// REFERENCE: https://man7.org/linux/man-pages/man2/futex.2.html
pub(super) fn start<'a>(
    process: &mut Process<'a>,
    index: usize,
    services: &Services<'_>,
    args: [usize; 6],
) -> Outcome<'a> {
    let restart = process.threads[index]
        .signals
        .borrow_mut()
        .restart
        .take()
        .filter(|restart| restart.arguments == args)
        .map(|restart| restart.deadline);
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
                let Wait {
                    deadline,
                    clock,
                    realtime: realtime_deadline,
                } = wait_deadline(process, services, command, timeout, realtime, restart)?;
                let registration = process.namespace.futexes.register(key, mask, frames);
                Ok(Outcome::Pending(Operation::Futex(Pending {
                    registration,
                    deadline,
                    clock,
                    realtime: realtime_deadline,
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

struct Wait {
    deadline: Deadline,
    clock: Option<devmgr::device::Pending<devmgr::rtc::Timestamp>>,
    realtime: Option<u64>,
}

fn wait_deadline(
    process: &Process<'_>,
    services: &Services<'_>,
    command: usize,
    timeout: usize,
    realtime: bool,
    restart: Option<(u64, bool)>,
) -> Result<Wait, Errno> {
    let mut duration = match restart {
        Some((target, target_realtime)) if target_realtime == realtime => {
            if realtime {
                Some(target)
            } else {
                Some(target.saturating_sub(wait::now()?))
            }
        }
        _ => wait::duration(process, timeout)?,
    };
    let mut realtime_deadline = None;
    let clock = if realtime {
        let clock = services.clock.as_ref().ok_or(EOPNOTSUPP)?;
        realtime_deadline = duration;
        duration
            .map(|_| clock.start_now().map_err(|_| EIO))
            .transpose()?
    } else {
        if command == 9 && restart.is_none() {
            let now = wait::now()?;
            duration = duration.map(|absolute| absolute.saturating_sub(now));
        }
        None
    };
    let deadline = Deadline::new(process.supervisor, if realtime { None } else { duration })?;
    Ok(Wait {
        deadline,
        clock,
        realtime: realtime_deadline,
    })
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
        self.clock.is_none()
    }

    pub fn restart_info(&self) -> Option<(u64, bool)> {
        self.realtime
            .map(|deadline| (deadline, true))
            .or_else(|| self.deadline.deadline().map(|deadline| (deadline, false)))
    }

    pub fn waits<'a>(&'a self, items: &mut alloc::vec::Vec<WaitItem<'a>>) {
        if let Some(clock) = &self.clock {
            items.push(WaitItem::new(clock.call(), runtime::ipc::READABLE));
        }
        self.deadline.waits(items);
    }

    pub fn poll(&mut self, process: &Process<'_>) -> Option<SyscallResult> {
        if self.registration.ready() {
            return Some(Ok(0));
        }
        if let Some(request) = &mut self.clock {
            let timestamp = match request.try_complete() {
                Err(devmgr::Error::Runtime(runtime::Error::Kernel(KernelError::WouldBlock))) => {
                    return None;
                }
                Err(_) => return Some(Err(EIO)),
                Ok(timestamp) => timestamp,
            };
            let now =
                i128::from(timestamp.seconds) * 1_000_000_000 + i128::from(timestamp.nanoseconds);
            let remaining = (i128::from(self.realtime.unwrap_or(0)) - now).max(0);
            self.clock = None;
            if remaining == 0 {
                return Some(Err(ETIMEDOUT));
            }
            let remaining = u64::try_from(remaining).unwrap_or(u64::MAX);
            match Deadline::new(process.supervisor, Some(remaining)) {
                Ok(deadline) => self.deadline = deadline,
                Err(error) => return Some(Err(error)),
            }
        }
        match self.deadline.expired() {
            Ok(true) => Some(Err(ETIMEDOUT)),
            Ok(false) => None,
            Err(error) => Some(Err(error)),
        }
    }
}
