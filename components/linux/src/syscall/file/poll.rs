// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! File descriptor readiness waiting.

use super::{Pending, outcome};
use crate::{
    file::OpenFileDescription,
    process::Process,
    syscall::{
        Outcome,
        abi::{EBADF, EINVAL, Errno, SyscallResult},
        wait::{self, Deadline},
    },
};
use alloc::{rc::Rc, vec::Vec};
use runtime::ipc::WaitItem;

// REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/fs/select.c?h=v6.12
const INPUT: u16 = 1 | 0x40;
const OUTPUT: u16 = 4 | 0x100;
const ERROR: u16 = 8;
const HANGUP: u16 = 0x10;
const INVALID: u16 = 0x20;

pub(in crate::syscall) struct Poll<'a> {
    entries: Vec<Entry<'a>>,
    output: Output,
    deadline: Deadline,
    timeout: usize,
}

struct Entry<'a> {
    description: Option<Rc<OpenFileDescription<'a>>>,
    events: u16,
    revents: u16,
    negative: bool,
    index: usize,
}

enum Output {
    Poll {
        address: usize,
        bytes: Vec<u8>,
    },
    Select {
        addresses: [usize; 3],
        bytes: usize,
        requested: [Vec<u8>; 3],
    },
}

pub(super) fn start<'a>(process: &Process<'a>, args: [usize; 6], select: bool) -> Outcome<'a> {
    outcome((|| {
        let (entries, output, timeout) = if select {
            let (entries, output) = select_entries(process, args)?;
            (entries, output, args[4])
        } else {
            let (entries, output) = poll_entries(process, args[0], args[1])?;
            (entries, output, args[2])
        };
        let deadline = Deadline::new(process.supervisor, wait::duration(process, timeout)?)?;
        Ok(Pending::Poll(Poll {
            entries,
            output,
            deadline,
            timeout,
        }))
    })())
}

fn poll_entries<'a>(
    process: &Process<'a>,
    address: usize,
    count: usize,
) -> Result<(Vec<Entry<'a>>, Output), Errno> {
    let length = count.checked_mul(8).ok_or(EINVAL)?;
    let bytes = wait::read_bytes(process, address, length)?;
    let entries = bytes
        .as_chunks::<8>()
        .0
        .iter()
        .enumerate()
        .map(|(index, bytes)| {
            let fd = i32::from_le_bytes(bytes[..4].try_into().unwrap());
            Entry {
                description: process
                    .files
                    .get(fd as usize)
                    .map(|fd| Rc::clone(&fd.description)),
                negative: fd < 0,
                events: u16::from_le_bytes(bytes[4..6].try_into().unwrap()),
                revents: 0,
                index,
            }
        })
        .collect();
    Ok((entries, Output::Poll { address, bytes }))
}

fn select_entries<'a>(
    process: &Process<'a>,
    args: [usize; 6],
) -> Result<(Vec<Entry<'a>>, Output), Errno> {
    let count = args[0] as i32;
    if count < 0 {
        return Err(EINVAL);
    }
    let length = (count as usize).div_ceil(64) * 8;
    let addresses: [usize; 3] = args[1..4].try_into().unwrap();
    let mut requested = [Vec::new(), Vec::new(), Vec::new()];
    for (set, address) in requested.iter_mut().zip(addresses) {
        if address != 0 {
            *set = wait::read_bytes(process, address, length)?;
        }
    }
    let mut entries = Vec::new();
    for fd in 0..count as usize {
        let mut events = 0;
        for (set, mask) in requested
            .iter()
            .zip([INPUT | ERROR | HANGUP, OUTPUT | ERROR, 2])
        {
            if set
                .get(fd / 8)
                .is_some_and(|byte| byte & (1 << (fd % 8)) != 0)
            {
                events |= mask;
            }
        }
        if events != 0 {
            entries.push(Entry {
                description: Some(Rc::clone(&process.files.get(fd).ok_or(EBADF)?.description)),
                events,
                revents: 0,
                negative: false,
                index: fd,
            });
        }
    }
    Ok((
        entries,
        Output::Select {
            addresses,
            bytes: length,
            requested,
        },
    ))
}

fn readiness(description: &OpenFileDescription<'_>, process: &Process<'_>, index: usize) -> u16 {
    match description {
        OpenFileDescription::Pipe { end, .. } => {
            let ready = end.readiness();
            ready | if ready & 1 != 0 { 0x40 } else { 0 } | if ready & 4 != 0 { 0x100 } else { 0 }
        }
        OpenFileDescription::Regular { .. } | OpenFileDescription::Directory { .. } => {
            INPUT | OUTPUT
        }
        OpenFileDescription::VirtualFile { .. } => INPUT,
        OpenFileDescription::Signalfd { mask, .. } => {
            if super::signalfd::ready(process, index, mask.get()) {
                INPUT
            } else {
                0
            }
        }
        OpenFileDescription::Timerfd { state, .. } => {
            if state
                .next
                .get()
                .is_some_and(|next| wait::now().is_ok_and(|now| now >= next))
            {
                INPUT
            } else {
                0
            }
        }
        OpenFileDescription::Eventfd { counter, .. } => {
            (if counter.get() < u64::MAX - 1 {
                OUTPUT
            } else {
                0
            }) | if counter.get() > 0 { INPUT } else { 0 }
        }
        OpenFileDescription::Terminal { terminal, .. } => {
            OUTPUT
                | if terminal.readable() { INPUT } else { 0 }
                | if terminal.failed() { ERROR | HANGUP } else { 0 }
        }
        OpenFileDescription::Virtual { .. } => INPUT | OUTPUT,
    }
}

impl Poll<'_> {
    pub fn interrupt(&self, process: &Process<'_>) -> SyscallResult {
        if self.timeout != 0 {
            wait::write_duration(process, self.timeout, self.deadline.remaining()?.unwrap())?;
        }
        Err(crate::syscall::abi::EINTR)
    }

    pub fn waits<'a>(&'a self, items: &mut Vec<WaitItem<'a>>) {
        self.deadline.waits(items);
        for entry in &self.entries {
            if let Some(OpenFileDescription::Timerfd { state, .. }) = entry.description.as_deref() {
                items.push(WaitItem::new(&state.event, 1));
            }
        }
    }

    pub fn poll(&mut self, process: &Process<'_>, index: usize) -> Option<SyscallResult> {
        let mut ready = 0;
        for entry in &mut self.entries {
            entry.revents = if entry.negative {
                0
            } else {
                entry.description.as_ref().map_or(INVALID, |description| {
                    readiness(description, process, index)
                        & (entry.events
                            | if matches!(self.output, Output::Poll { .. }) {
                                ERROR | HANGUP
                            } else {
                                0
                            })
                })
            };
            ready += usize::from(entry.revents != 0);
        }
        if ready == 0 {
            match self.deadline.expired() {
                Ok(false) => return None,
                Err(error) => return Some(Err(error)),
                Ok(true) => {}
            }
        }
        Some(self.finish(process, ready))
    }

    fn finish(&mut self, process: &Process<'_>, ready: usize) -> SyscallResult {
        let count = match &mut self.output {
            Output::Poll { address, bytes } => {
                for entry in &self.entries {
                    bytes[entry.index * 8 + 6..entry.index * 8 + 8]
                        .copy_from_slice(&entry.revents.to_le_bytes());
                }
                wait::write_bytes(process, *address, bytes)?;
                ready
            }
            Output::Select {
                addresses,
                bytes,
                requested,
            } => {
                let mut count = 0;
                for ((address, mask), requested) in addresses
                    .iter()
                    .zip([INPUT | ERROR | HANGUP, OUTPUT | ERROR, 2])
                    .zip(requested)
                {
                    if *address == 0 {
                        continue;
                    }
                    let mut output = alloc::vec![0; *bytes];
                    for entry in &self.entries {
                        if requested[entry.index / 8] & (1 << (entry.index % 8)) != 0
                            && entry.revents & mask != 0
                        {
                            output[entry.index / 8] |= 1 << (entry.index % 8);
                            count += 1;
                        }
                    }
                    wait::write_bytes(process, *address, &output)?;
                }
                count
            }
        };
        if self.timeout != 0 {
            wait::write_duration(process, self.timeout, self.deadline.remaining()?.unwrap())?;
        }
        Ok(count)
    }
}
