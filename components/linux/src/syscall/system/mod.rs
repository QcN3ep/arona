// SPDX-License-Identifier: MPL-2.0

//! 时钟、熵源与系统信息。

mod clock;
mod random;

use super::{
    Operation, Outcome,
    abi::{self, EFAULT, Errno, SyscallResult},
    wait::Deadline,
};
use crate::{Services, elf::Machine, process::Process};
use alloc::vec::Vec;
use runtime::ipc::WaitItem;

pub(super) enum Pending {
    Clock(clock::Pending),
    Sleep {
        deadline: Deadline,
        remaining: usize,
    },
    Random(random::Pending),
}

pub(super) fn dispatch<'a>(
    process: &Process<'a>,
    services: &Services<'_>,
    number: usize,
    args: [usize; 6],
) -> Option<Outcome<'a>> {
    Some(match number {
        abi::UNAME => Outcome::Complete(uname(process, args[0])),
        abi::CLOCK_GETTIME
        | abi::CLOCK_GETRES
        | abi::GETTIMEOFDAY
        | abi::NANOSLEEP
        | abi::CLOCK_NANOSLEEP => clock::dispatch(process, services, number, args),
        abi::GETRANDOM => random::start(process, services, args),
        _ => return None,
    })
}

fn outcome<'a>(pending: Result<Pending, Errno>) -> Outcome<'a> {
    match pending {
        Ok(pending) => Outcome::Pending(Operation::System(pending)),
        Err(error) => Outcome::Complete(Err(error)),
    }
}

// REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/include/uapi/linux/utsname.h?h=v6.12
fn uname(process: &Process<'_>, address: usize) -> SyscallResult {
    let mut bytes = [0; 6 * 65];
    let machine: &[u8] = match process.machine {
        Machine::Riscv64 => b"riscv64",
        Machine::LoongArch64 => b"loongarch64",
    };
    for (field, value) in bytes.as_chunks_mut::<65>().0.iter_mut().zip([
        b"Linux".as_slice(),
        b"arona",
        env!("CARGO_PKG_VERSION").as_bytes(),
        b"Arona",
        machine,
        b"(none)",
    ]) {
        field[..value.len()].copy_from_slice(value);
    }
    process
        .address_space
        .write(address, &bytes)
        .map_err(|_| EFAULT)?;
    Ok(0)
}

impl Pending {
    pub fn interrupt(&self, process: &Process<'_>) -> Option<SyscallResult> {
        match self {
            Self::Sleep {
                deadline,
                remaining,
            } => Some((|| {
                if *remaining != 0 {
                    super::wait::write_duration(
                        process,
                        *remaining,
                        deadline.remaining()?.unwrap(),
                    )?;
                }
                Err(abi::EINTR)
            })()),
            Self::Clock(clock) if clock.sleeping() => Some(Err(abi::EINTR)),
            _ => None,
        }
    }

    pub fn waits<'a>(&'a self, items: &mut Vec<WaitItem<'a>>) {
        match self {
            Self::Clock(clock) => clock.waits(items),
            Self::Random(random) => {
                items.push(WaitItem::new(random.call(), runtime::ipc::READABLE))
            }
            Self::Sleep { deadline, .. } => deadline.waits(items),
        }
    }
    pub fn poll(&mut self, process: &Process<'_>) -> Option<SyscallResult> {
        match self {
            Self::Clock(clock) => clock.poll(process),
            Self::Random(random) => random.poll(process),
            Self::Sleep { deadline, .. } => match deadline.expired() {
                Ok(true) => Some(Ok(0)),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            },
        }
    }
}
