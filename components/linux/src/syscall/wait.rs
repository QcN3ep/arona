// SPDX-License-Identifier: MPL-2.0

//! Syscall deadlines and timer waiting.

use super::abi::{EFAULT, EINVAL, EIO, ENOMEM, Errno};
use crate::process::Process;
use alloc::vec::Vec;
use runtime::{Event, Handle, Rights, Timer, ipc::WaitItem};

pub(super) struct Deadline {
    end: Option<u64>,
    event: Option<Handle<Event>>,
    _timer: Option<Handle<Timer>>,
}

impl Deadline {
    pub fn new(task: &Handle<runtime::Task>, duration: Option<u64>) -> Result<Self, Errno> {
        let end = duration
            .map(|duration| now().map(|now| now.saturating_add(duration)))
            .transpose()?;
        let (event, timer) = if duration.is_some_and(|duration| duration != 0) {
            let event = task.event(Rights::WAIT | Rights::SIGNAL).map_err(error)?;
            let timer = task.timer(&event, 1, Rights::WRITE).map_err(error)?;
            timer.arm(end.unwrap()).map_err(error)?;
            (Some(event), Some(timer))
        } else {
            (None, None)
        };
        Ok(Self {
            end,
            event,
            _timer: timer,
        })
    }

    pub fn expired(&self) -> Result<bool, Errno> {
        self.end
            .map_or(Ok(false), |end| now().map(|now| now >= end))
    }

    pub fn remaining(&self) -> Result<Option<u64>, Errno> {
        self.end
            .map(|end| now().map(|now| end.saturating_sub(now)))
            .transpose()
    }

    pub fn waits<'a>(&'a self, items: &mut Vec<WaitItem<'a>>) {
        if let Some(event) = &self.event {
            items.push(WaitItem::new(event, 1));
        }
    }
}

pub(super) fn now() -> Result<u64, Errno> {
    runtime::thread::now_ns().map_err(error)
}

pub(super) fn duration(process: &Process<'_>, address: usize) -> Result<Option<u64>, Errno> {
    if address == 0 {
        return Ok(None);
    }
    let mut bytes = [0; 16];
    process
        .address_space
        .read(address, &mut bytes)
        .map_err(|_| EFAULT)?;
    let seconds = i64::from_le_bytes(bytes[..8].try_into().unwrap());
    let nanoseconds = i64::from_le_bytes(bytes[8..].try_into().unwrap());
    if seconds < 0 || !(0..1_000_000_000).contains(&nanoseconds) {
        return Err(EINVAL);
    }
    Ok(Some(
        (seconds as u64)
            .saturating_mul(1_000_000_000)
            .saturating_add(nanoseconds as u64),
    ))
}

pub(super) fn write_duration(
    process: &Process<'_>,
    address: usize,
    duration: u64,
) -> Result<(), Errno> {
    let mut bytes = [0; 16];
    bytes[..8].copy_from_slice(&(duration / 1_000_000_000).to_le_bytes());
    bytes[8..].copy_from_slice(&(duration % 1_000_000_000).to_le_bytes());
    process
        .address_space
        .write(address, &bytes)
        .map_err(|_| EFAULT)
}

pub(super) fn read_bytes(
    process: &Process<'_>,
    address: usize,
    length: usize,
) -> Result<Vec<u8>, Errno> {
    if length > isize::MAX as usize || address.checked_add(length).is_none() {
        return Err(EFAULT);
    }
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length).map_err(|_| ENOMEM)?;
    bytes.resize(length, 0);
    for (index, chunk) in bytes.chunks_mut(4096).enumerate() {
        process
            .address_space
            .read(address + index * 4096, chunk)
            .map_err(|_| EFAULT)?;
    }
    Ok(bytes)
}

pub(super) fn write_bytes(
    process: &Process<'_>,
    address: usize,
    bytes: &[u8],
) -> Result<(), Errno> {
    address.checked_add(bytes.len()).ok_or(EFAULT)?;
    for (index, chunk) in bytes.chunks(4096).enumerate() {
        process
            .address_space
            .write(address + index * 4096, chunk)
            .map_err(|_| EFAULT)?;
    }
    Ok(())
}

pub(super) fn error(error: runtime::Error) -> Errno {
    match error {
        runtime::Error::Kernel(runtime::KernelError::NoMemory) => ENOMEM,
        _ => EIO,
    }
}
