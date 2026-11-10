// SPDX-License-Identifier: MPL-2.0

//! 子进程退出等待与回收。

use crate::{
    process::Process,
    syscall::{
        Operation, Outcome,
        abi::{self, ECHILD, EFAULT, EINVAL, EOPNOTSUPP, SyscallResult},
    },
};

pub(in crate::syscall) struct Pending {
    selector: Selector,
    output: usize,
    options: u32,
    info: bool,
}

enum Selector {
    All,
    Pid(usize),
    Group(usize),
}

// REFERENCE: https://man7.org/linux/man-pages/man2/wait.2.html
pub(super) fn start<'a>(process: &Process<'a>, number: usize, args: [usize; 6]) -> Outcome<'a> {
    let result = (|| {
        let info = number == abi::WAITID;
        let (selector, output, options, usage) = if info {
            let selector = match args[0] as u32 {
                0 => Selector::All,
                1 if args[1] != 0 => Selector::Pid(args[1] as u32 as usize),
                2 => Selector::Group(if args[1] == 0 {
                    group(process)
                } else {
                    args[1] as u32 as usize
                }),
                _ => return Err(EINVAL),
            };
            (selector, args[2], args[3] as u32, args[4])
        } else {
            let selector = match args[0] as i32 {
                -1 => Selector::All,
                0 => Selector::Group(group(process)),
                pid if pid > 0 => Selector::Pid(pid as usize),
                group => Selector::Group(group.unsigned_abs() as usize),
            };
            (selector, args[1], args[2] as u32, args[3])
        };
        let supported = if info {
            1 | 2 | 4 | 8 | 0x1000000
        } else {
            1 | 2 | 8
        };
        if options & !supported != 0 || info && options & (2 | 4 | 8) == 0 {
            return Err(EINVAL);
        }
        if usage != 0 {
            // TODO: 接入进程资源用量统计。
            return Err(EOPNOTSUPP);
        }
        Ok(Pending {
            selector,
            output,
            options,
            info,
        })
    })();
    match result {
        Ok(pending) => Outcome::Pending(Operation::WaitChild(pending)),
        Err(error) => Outcome::Complete(Err(error)),
    }
}

fn group(process: &Process<'_>) -> usize {
    process
        .namespace
        .processes
        .borrow()
        .get(&process.pid)
        .map_or(process.pid, |record| record.group)
}

impl Pending {
    pub fn poll(&self, process: &Process<'_>) -> Option<SyscallResult> {
        let records = process.namespace.processes.borrow();
        let mut child = false;
        let mut dead = None;
        for (&pid, record) in records.iter() {
            let matches = match self.selector {
                Selector::All => true,
                Selector::Pid(target) => target == pid,
                Selector::Group(group) => group == record.group,
            };
            if record.parent != process.pid || !matches {
                continue;
            }
            child = true;
            if let Some(status) = record.status
                && (!self.info || self.options & 4 != 0)
            {
                dead = Some((pid, status));
                break;
            }
        }
        drop(records);
        if let Some((pid, status)) = dead {
            let result = self.finish(process, pid, status);
            if result.is_ok() && self.options & 0x1000000 == 0 {
                process.namespace.processes.borrow_mut().remove(&pid);
            }
            Some(result)
        } else if !child {
            Some(Err(ECHILD))
        } else if self.options & 1 != 0 {
            Some(self.finish(process, 0, crate::ExitStatus { code: 0, signal: 0 }))
        } else {
            None
        }
    }

    fn finish(
        &self,
        process: &Process<'_>,
        pid: usize,
        status: crate::ExitStatus,
    ) -> SyscallResult {
        if self.info {
            let mut bytes = [0; 128];
            if pid != 0 {
                bytes[..4].copy_from_slice(&17u32.to_le_bytes());
                bytes[8..12]
                    .copy_from_slice(&(if status.signal == 0 { 1u32 } else { 2 }).to_le_bytes());
                bytes[16..20].copy_from_slice(&(pid as u32).to_le_bytes());
                bytes[24..28].copy_from_slice(
                    &u32::from(if status.signal == 0 {
                        status.code
                    } else {
                        status.signal
                    })
                    .to_le_bytes(),
                );
            }
            process
                .address_space
                .write(self.output, &bytes)
                .map_err(|_| EFAULT)?;
            Ok(0)
        } else {
            if self.output != 0 && pid != 0 {
                process
                    .address_space
                    .write(self.output, &(status.encode() as u32).to_le_bytes())
                    .map_err(|_| EFAULT)?;
            }
            Ok(pid)
        }
    }
}
