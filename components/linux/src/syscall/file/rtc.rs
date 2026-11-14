// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Linux RTC character device paths and `rtc_time` conversion.

use super::Buffer;
use crate::{
    file::OpenFileDescription,
    process::Process,
    syscall::{
        Outcome,
        abi::{
            EACCES, EBADF, EFAULT, EINVAL, EIO, ENOTTY, EOPNOTSUPP, EPERM, ESPIPE, Errno,
            SyscallResult,
        },
    },
};
use devmgr::rtc::{Calendar, Timestamp};

// REFERENCE: https://github.com/torvalds/linux/blob/v6.12/include/uapi/linux/rtc.h
const RTC_AIE_ON: u32 = 0x0000_7001;
const RTC_AIE_OFF: u32 = 0x0000_7002;
const RTC_UIE_ON: u32 = 0x0000_7003;
const RTC_UIE_OFF: u32 = 0x0000_7004;
const RTC_PIE_ON: u32 = 0x0000_7005;
const RTC_PIE_OFF: u32 = 0x0000_7006;
const RTC_WIE_ON: u32 = 0x0000_700f;
const RTC_WIE_OFF: u32 = 0x0000_7010;
const RTC_RD_TIME: u32 = 0x8024_7009;
const RTC_SET_TIME: u32 = 0x4024_700a;
const RTC_TIME_SIZE: usize = 36;
const IOCTL_TIMEOUT_NS: u64 = 1_000_000_000;

pub(super) fn is_rtc(process: &Process<'_>, raw: usize) -> bool {
    process
        .files
        .get(raw as i32 as usize)
        .is_some_and(|descriptor| {
            matches!(
                descriptor.description.as_ref(),
                OpenFileDescription::Rtc { .. }
            )
        })
}

pub(super) fn read<'a>(
    process: &Process<'a>,
    raw: usize,
    buffer: Buffer,
    offset: Option<usize>,
    services: &crate::Services<'_>,
) -> Outcome<'a> {
    Outcome::Complete((|| {
        let descriptor = process.files.get(super::fd(raw)?).ok_or(EBADF)?;
        if !matches!(
            descriptor.description.as_ref(),
            OpenFileDescription::Rtc { .. }
        ) {
            return Err(ENOTTY);
        }
        if offset.is_some() {
            return Err(ESPIPE);
        }
        if descriptor.description.flags() & 3 == 1 {
            return Err(EBADF);
        }
        let buffers = buffer.import(process)?;
        if buffers.total_length() < RTC_TIME_SIZE {
            return Err(EINVAL);
        }
        let time = now(services)?;
        let mut bytes = rtc_time(time)?;
        buffers.scatter(process, &mut bytes)?;
        Ok(RTC_TIME_SIZE)
    })())
}

pub(super) fn write(process: &Process<'_>, raw: usize) -> SyscallResult {
    let descriptor = process.files.get(super::fd(raw)?).ok_or(EBADF)?;
    if !matches!(
        descriptor.description.as_ref(),
        OpenFileDescription::Rtc { .. }
    ) {
        return Err(ENOTTY);
    }
    Err(EINVAL)
}

pub(super) fn ioctl(
    process: &Process<'_>,
    services: &crate::Services<'_>,
    raw: usize,
    command: usize,
    address: usize,
) -> SyscallResult {
    let descriptor = process.files.get(super::fd(raw)?).ok_or(EBADF)?;
    if !matches!(
        descriptor.description.as_ref(),
        OpenFileDescription::Rtc { .. }
    ) {
        return Err(ENOTTY);
    }
    match command as u32 {
        RTC_RD_TIME => {
            let time = now(services)?;
            let bytes = rtc_time(time)?;
            process
                .address_space
                .write(address, &bytes)
                .map_err(|_| EFAULT)?;
            Ok(0)
        }
        RTC_SET_TIME => {
            if process.credentials.euid != 0 {
                return Err(EPERM);
            }
            let time = read_rtc_time(process, address)?;
            set(services, time)?;
            Ok(0)
        }
        RTC_AIE_ON | RTC_AIE_OFF | RTC_UIE_ON | RTC_UIE_OFF | RTC_PIE_ON | RTC_PIE_OFF
        | RTC_WIE_ON | RTC_WIE_OFF => Err(EINVAL),
        // TODO: Integrate alarm, wakealarm, periodic interrupt, and epoch ioctls.
        _ => Err(ENOTTY),
    }
}

fn now(services: &crate::Services<'_>) -> Result<Timestamp, Errno> {
    let clock = services.clock.as_ref().ok_or(EOPNOTSUPP)?;
    let deadline = runtime::thread::now_ns()
        .map_err(|_| EIO)?
        .saturating_add(IOCTL_TIMEOUT_NS);
    clock.now(deadline).map_err(call_error)
}

fn set(services: &crate::Services<'_>, time: Timestamp) -> Result<(), Errno> {
    let clock = services.clock.as_ref().ok_or(EOPNOTSUPP)?;
    let deadline = runtime::thread::now_ns()
        .map_err(|_| EIO)?
        .saturating_add(IOCTL_TIMEOUT_NS);
    clock.set(time, deadline).map_err(call_error)
}

fn call_error(error: devmgr::Error) -> Errno {
    match error {
        devmgr::Error::InvalidRequest => EINVAL,
        devmgr::Error::Unsupported => EOPNOTSUPP,
        devmgr::Error::Denied => EACCES,
        _ => EIO,
    }
}

fn read_rtc_time(process: &Process<'_>, address: usize) -> Result<Timestamp, Errno> {
    let mut bytes = [0; RTC_TIME_SIZE];
    process
        .address_space
        .read(address, &mut bytes)
        .map_err(|_| EFAULT)?;
    let field = |offset: usize| i32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    let year = field(20).checked_add(1900).ok_or(EINVAL)?;
    let month = field(16).checked_add(1).ok_or(EINVAL)?;
    let day = field(12);
    let hour = field(8);
    let minute = field(4);
    let second = field(0);
    Timestamp::from_calendar(
        year,
        u32::try_from(month).map_err(|_| EINVAL)?,
        u32::try_from(day).map_err(|_| EINVAL)?,
        u32::try_from(hour).map_err(|_| EINVAL)?,
        u32::try_from(minute).map_err(|_| EINVAL)?,
        u32::try_from(second).map_err(|_| EINVAL)?,
    )
    .map_err(|_| EINVAL)
}

fn rtc_time(time: Timestamp) -> Result<[u8; RTC_TIME_SIZE], Errno> {
    let calendar = time.calendar().map_err(|_| EIO)?;
    Ok(encode(calendar))
}

fn encode(calendar: Calendar) -> [u8; RTC_TIME_SIZE] {
    let fields = [
        i32::from(calendar.second),
        i32::from(calendar.minute),
        i32::from(calendar.hour),
        i32::from(calendar.day),
        i32::from(calendar.month) - 1,
        calendar.year - 1900,
        i32::from(calendar.weekday),
        i32::from(calendar.year_day),
        -1,
    ];
    let mut bytes = [0; RTC_TIME_SIZE];
    for (output, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(fields) {
        output.copy_from_slice(&value.to_le_bytes());
    }
    bytes
}
