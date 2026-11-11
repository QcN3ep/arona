// SPDX-License-Identifier: MPL-2.0

//! Linux time structure encoding and sleep.

use super::{Pending as Operation, outcome};
use crate::{
    Services,
    process::Process,
    syscall::{
        Outcome,
        abi::{self, EFAULT, EINVAL, EIO, EOPNOTSUPP, SyscallResult},
        wait::{self, Deadline},
    },
};
use runtime::{Call, Handle};

pub(in crate::syscall) struct Pending {
    request: devmgr::device::Pending<devmgr::rtc::Timestamp>,
    output: Output,
}

enum Output {
    Time {
        address: usize,
        timeval: bool,
        zone: usize,
    },
    Sleep {
        deadline: u64,
        timer: Option<Deadline>,
    },
}

// REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/kernel/time/posix-stubs.c?h=v6.12
pub(super) fn dispatch<'a>(
    process: &Process<'a>,
    services: &Services<'_>,
    number: usize,
    args: [usize; 6],
) -> Outcome<'a> {
    let [clock, address, ..] = args;
    if number == abi::NANOSLEEP || number == abi::CLOCK_NANOSLEEP {
        return sleep(process, services, number, args);
    }
    if number == abi::GETTIMEOFDAY {
        return realtime(
            services,
            Output::Time {
                address: clock,
                timeval: true,
                zone: address,
            },
        );
    }
    if !matches!(clock as i32, 0 | 1 | 4 | 5 | 6 | 7) {
        return Outcome::Complete(Err(EINVAL));
    }
    let realtime_clock = matches!(clock as i32, 0 | 5);
    if number == abi::CLOCK_GETRES {
        return Outcome::Complete(if address == 0 {
            Ok(0)
        } else {
            write_time(
                process,
                address,
                if realtime_clock { 1_000_000_000 } else { 1 },
                false,
            )
        });
    }
    if realtime_clock {
        realtime(
            services,
            Output::Time {
                address,
                timeval: false,
                zone: 0,
            },
        )
    } else {
        Outcome::Complete(wait::now().and_then(|time| write_time(process, address, time, false)))
    }
}

fn realtime<'a>(services: &Services<'_>, output: Output) -> Outcome<'a> {
    outcome((|| {
        let clock = services.clock.as_ref().ok_or(EOPNOTSUPP)?;
        Ok(Operation::Clock(Pending {
            request: clock.start_now().map_err(|_| EIO)?,
            output,
        }))
    })())
}

fn sleep<'a>(
    process: &Process<'a>,
    services: &Services<'_>,
    number: usize,
    args: [usize; 6],
) -> Outcome<'a> {
    let (clock, flags, address) = if number == abi::NANOSLEEP {
        (1, 0, args[0])
    } else {
        (args[0] as i32, args[1] as i32, args[2])
    };
    if !matches!(clock, 0 | 1 | 7) || flags & !1 != 0 {
        return Outcome::Complete(Err(EINVAL));
    }
    let duration = match wait::duration(process, address) {
        Ok(Some(duration)) => duration,
        Ok(None) => return Outcome::Complete(Err(EFAULT)),
        Err(error) => return Outcome::Complete(Err(error)),
    };
    if flags == 1 && clock == 0 {
        return realtime(
            services,
            Output::Sleep {
                deadline: duration,
                timer: None,
            },
        );
    }
    outcome(
        wait::now()
            .and_then(|now| {
                Deadline::new(
                    process.supervisor,
                    Some(if flags == 1 {
                        duration.saturating_sub(now)
                    } else {
                        duration
                    }),
                )
            })
            .map(|deadline| Operation::Sleep {
                deadline,
                remaining: if flags == 1 {
                    0
                } else if number == abi::NANOSLEEP {
                    args[1]
                } else {
                    args[3]
                },
            }),
    )
}

impl Pending {
    pub fn sleeping(&self) -> bool {
        matches!(self.output, Output::Sleep { .. })
    }
    pub fn call(&self) -> &Handle<Call> {
        self.request.call()
    }
    pub fn poll(&mut self, process: &Process<'_>) -> Option<SyscallResult> {
        if let Output::Sleep {
            timer: Some(timer), ..
        } = &self.output
        {
            return match timer.expired() {
                Ok(true) => Some(Ok(0)),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            };
        }
        let timestamp = match self.request.try_complete() {
            Err(devmgr::Error::Runtime(runtime::Error::Kernel(
                runtime::KernelError::WouldBlock,
            ))) => return None,
            Err(_) => return Some(Err(EIO)),
            Ok(timestamp) => timestamp,
        };
        match &mut self.output {
            Output::Time {
                address,
                timeval,
                zone,
            } => Some((|| {
                if *address != 0 || !*timeval {
                    write_timestamp(process, *address, timestamp, *timeval)?;
                }
                if *timeval && *zone != 0 {
                    process
                        .address_space
                        .write(*zone, &[0; 8])
                        .map_err(|_| EFAULT)?;
                }
                Ok(0)
            })()),
            Output::Sleep { deadline, timer } => {
                let now = i128::from(timestamp.seconds) * 1_000_000_000
                    + i128::from(timestamp.nanoseconds);
                let remaining =
                    u64::try_from((i128::from(*deadline) - now).max(0)).unwrap_or(u64::MAX);
                match Deadline::new(process.supervisor, Some(remaining)) {
                    Ok(deadline) => {
                        *timer = Some(deadline);
                        None
                    }
                    Err(error) => Some(Err(error)),
                }
            }
        }
    }
    pub fn waits<'a>(&'a self, items: &mut alloc::vec::Vec<runtime::ipc::WaitItem<'a>>) {
        if let Output::Sleep {
            timer: Some(timer), ..
        } = &self.output
        {
            timer.waits(items);
        } else {
            items.push(runtime::ipc::WaitItem::new(
                self.call(),
                runtime::ipc::READABLE,
            ));
        }
    }
}

fn write_time(process: &Process<'_>, address: usize, time: u64, timeval: bool) -> SyscallResult {
    let mut bytes = [0; 16];
    bytes[..8].copy_from_slice(&(time / 1_000_000_000).to_le_bytes());
    bytes[8..]
        .copy_from_slice(&(time % 1_000_000_000 / if timeval { 1000 } else { 1 }).to_le_bytes());
    process
        .address_space
        .write(address, &bytes)
        .map_err(|_| EFAULT)?;
    Ok(0)
}

fn write_timestamp(
    process: &Process<'_>,
    address: usize,
    timestamp: devmgr::rtc::Timestamp,
    timeval: bool,
) -> SyscallResult {
    let mut bytes = [0; 16];
    bytes[..8].copy_from_slice(&timestamp.seconds.to_le_bytes());
    let fraction = u64::from(timestamp.nanoseconds) / if timeval { 1000 } else { 1 };
    bytes[8..].copy_from_slice(&fraction.to_le_bytes());
    process
        .address_space
        .write(address, &bytes)
        .map_err(|_| EFAULT)?;
    Ok(0)
}
