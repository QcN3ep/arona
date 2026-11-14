// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Read-only virtual files mounted at `/proc` and `/sys`.

use super::{Buffer, fd, net};
use crate::{
    Services,
    file::OpenFileDescription,
    memory::Origin,
    process::Process,
    syscall::abi::{EBADF, EINVAL, EIO, ENOENT, EOVERFLOW, EROFS, Errno, SyscallResult},
};
use alloc::{string::String, vec::Vec};
use core::fmt::Write;
use runtime::memory::Permissions;

/// Clock ticks per second reported by `/proc/<pid>/stat`.
const USER_HZ: u64 = 100;
const NANOS_PER_TICK: u64 = 1_000_000_000 / USER_HZ;

#[derive(Clone, Copy)]
pub(super) enum File {
    ProcSelfStatus,
    ProcSelfMaps,
    ProcSelfSmaps,
    ProcMeminfo,
    ProcUptime,
    ProcVersion,
    ProcMounts,
    ProcCmdline(usize),
    ProcStat(usize),
    CpuOnline,
    OsRelease,
    Net(net::File),
}

impl File {
    pub fn inode(self) -> u64 {
        match self {
            Self::ProcSelfStatus => 0x7072_0001,
            Self::ProcCmdline(pid) => 0x7072_1000 + pid as u64,
            Self::ProcVersion => 0x7072_0003,
            Self::ProcMounts => 0x7072_0004,
            Self::ProcMeminfo => 0x7072_0005,
            Self::ProcUptime => 0x7072_0006,
            Self::ProcSelfMaps => 0x7072_0007,
            Self::ProcSelfSmaps => 0x7072_0008,
            Self::ProcStat(pid) => 0x7072_2000 + pid as u64,
            Self::CpuOnline => 0x7379_0001,
            Self::OsRelease => 0x7379_0002,
            Self::Net(file) => file.inode(),
        }
    }

    pub fn content(self, process: &Process<'_>, services: &Services<'_>) -> Result<Vec<u8>, Errno> {
        Ok(match self {
            Self::ProcSelfStatus => status(process),
            Self::ProcSelfMaps => maps(process, false)?,
            Self::ProcSelfSmaps => maps(process, true)?,
            Self::ProcMeminfo => meminfo(process)?,
            Self::ProcUptime => uptime()?,
            Self::ProcVersion => alloc::format!(
                "Linux version {} (arona) #1 SMP\n",
                env!("CARGO_PKG_VERSION")
            )
            .into_bytes(),
            Self::ProcMounts => concat!(
                "proc /proc proc ro,relatime 0 0\n",
                "sysfs /sys sysfs ro,relatime 0 0\n",
                "mqueue /dev/mqueue mqueue rw,nosuid,nodev,noexec,relatime 0 0\n",
            )
            .as_bytes()
            .to_vec(),
            Self::ProcCmdline(pid) => cmdline(process, pid)?,
            Self::ProcStat(pid) => stat(process, pid)?,
            Self::CpuOnline => b"0\n".to_vec(),
            Self::OsRelease => alloc::format!("{}\n", env!("CARGO_PKG_VERSION")).into_bytes(),
            Self::Net(file) => return net::content(file, services),
        })
    }
}

pub(super) fn resolve(process: &Process<'_>, path: &[u8]) -> Option<File> {
    Some(match path {
        b"/proc/self/status" => File::ProcSelfStatus,
        b"/proc/self/cmdline" => File::ProcCmdline(process.pid),
        b"/proc/self/maps" => File::ProcSelfMaps,
        b"/proc/self/smaps" => File::ProcSelfSmaps,
        b"/proc/meminfo" => File::ProcMeminfo,
        b"/proc/uptime" => File::ProcUptime,
        b"/proc/version" => File::ProcVersion,
        b"/proc/mounts" => File::ProcMounts,
        b"/sys/devices/system/cpu/online" => File::CpuOnline,
        b"/sys/kernel/osrelease" => File::OsRelease,
        b"/proc/net/dev" => File::Net(net::File::Dev),
        b"/proc/net/route" => File::Net(net::File::Route),
        b"/proc/net/tcp" => File::Net(net::File::Tcp),
        b"/proc/net/tcp6" => File::Net(net::File::Tcp6),
        b"/proc/net/udp" => File::Net(net::File::Udp),
        b"/proc/net/udp6" => File::Net(net::File::Udp6),
        b"/proc/net/unix" => File::Net(net::File::Unix),
        _ => return resolve_pid(process, path),
    })
}

fn resolve_pid(process: &Process<'_>, path: &[u8]) -> Option<File> {
    let (id, name) = split_path(path.strip_prefix(b"/proc/")?)?;
    let id = if id == b"self" {
        process.pid
    } else {
        parse_pid(id)?
    };
    let id = process.namespace.process_id(id)?;
    match name {
        b"cmdline" => Some(File::ProcCmdline(id)),
        b"stat" => Some(File::ProcStat(id)),
        _ => None,
    }
}

fn split_path(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let index = bytes.iter().position(|byte| *byte == b'/')?;
    Some((&bytes[..index], &bytes[index + 1..]))
}

fn parse_pid(bytes: &[u8]) -> Option<usize> {
    if bytes.is_empty() {
        return None;
    }
    let mut value = 0usize;
    for byte in bytes {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value
            .checked_mul(10)?
            .checked_add(usize::from(byte - b'0'))?;
    }
    (value != 0).then_some(value)
}

/// Resolve the read-only link target for `readlinkat` paths under `/proc`.
pub(super) fn link(process: &Process<'_>, path: &[u8]) -> Option<Vec<u8>> {
    let id = match path {
        b"/proc/self/exe" | b"/proc/thread-self/exe" => process.pid,
        _ => {
            let (id, name) = split_path(path.strip_prefix(b"/proc/")?)?;
            if name != b"exe" {
                return None;
            }
            if id == b"self" {
                process.pid
            } else {
                parse_pid(id)?
            }
        }
    };
    let records = process.namespace.processes.borrow();
    let record = records.get(&id)?;
    (!record.program.is_empty()).then(|| record.program.clone())
}

pub(super) fn is_virtual(process: &Process<'_>, raw: usize) -> bool {
    fd(raw)
        .ok()
        .and_then(|fd| process.files.get(fd))
        .is_some_and(|descriptor| {
            matches!(
                descriptor.description.as_ref(),
                OpenFileDescription::VirtualFile { .. }
            )
        })
}

pub(super) fn read(
    process: &Process<'_>,
    raw: usize,
    buffer: Buffer,
    offset: Option<usize>,
) -> SyscallResult {
    let descriptor = process.files.get(fd(raw)?).ok_or(EBADF)?;
    let OpenFileDescription::VirtualFile {
        data,
        offset: position,
        ..
    } = descriptor.description.as_ref()
    else {
        return Err(EBADF);
    };
    if descriptor.description.flags() & 3 == 1 || descriptor.description.path_only() {
        return Err(EBADF);
    }
    if offset.is_some_and(|offset| offset > i64::MAX as usize) {
        return Err(EINVAL);
    }
    let buffers = buffer.import(process)?;
    let start = offset.unwrap_or_else(|| position.get());
    let length = data.len().saturating_sub(start).min(buffers.length());
    if length == 0 {
        return Ok(0);
    }
    let mut bytes = data[start..start + length].to_vec();
    let count = buffers.scatter(process, &mut bytes)?;
    if offset.is_none() {
        position.set(start + count);
    }
    Ok(count)
}

pub(super) fn write(process: &Process<'_>, raw: usize) -> SyscallResult {
    process.files.get(fd(raw)?).ok_or(EBADF)?;
    Err(EROFS)
}

pub(super) fn seek(
    process: &Process<'_>,
    raw: usize,
    displacement: usize,
    whence: usize,
) -> SyscallResult {
    let descriptor = process.files.get(fd(raw)?).ok_or(EBADF)?;
    let OpenFileDescription::VirtualFile { data, offset, .. } = descriptor.description.as_ref()
    else {
        return Err(EBADF);
    };
    if descriptor.description.path_only() {
        return Err(EBADF);
    }
    let base = match whence {
        0 => 0,
        1 => offset.get(),
        2 => data.len(),
        _ => return Err(EINVAL),
    };
    let position = base as i128 + i128::from(displacement as i64);
    if position < 0 {
        return Err(EINVAL);
    }
    if position > i128::from(i64::MAX) {
        return Err(EOVERFLOW);
    }
    let position = position as usize;
    offset.set(position);
    Ok(position)
}

fn status(process: &Process<'_>) -> Vec<u8> {
    let name = command_name(&process.program);
    alloc::format!(
        "Name:\t{}\nState:\tR (running)\nTgid:\t{pid}\nPid:\t{pid}\nPPid:\t{}\nUid:\t0\t0\t0\t0\nGid:\t0\t0\t0\t0\nThreads:\t{}\n",
        String::from_utf8_lossy(&name),
        process.namespace.parent(process.pid),
        process.threads.len(),
        pid = process.pid,
    )
    .into_bytes()
}

fn command_name(program: &[u8]) -> Vec<u8> {
    let name = program
        .rsplit(|byte| *byte == b'/')
        .next()
        .unwrap_or(program);
    name[..name.len().min(15)].to_vec()
}

fn cmdline(process: &Process<'_>, pid: usize) -> Result<Vec<u8>, Errno> {
    let records = process.namespace.processes.borrow();
    let record = records.get(&pid).ok_or(ENOENT)?;
    let mut data = Vec::new();
    if record.argv.is_empty() {
        data.extend_from_slice(&record.program);
        data.push(0);
        return Ok(data);
    }
    for argument in &record.argv {
        data.extend_from_slice(argument);
        data.push(0);
    }
    Ok(data)
}

fn stat(process: &Process<'_>, pid: usize) -> Result<Vec<u8>, Errno> {
    let records = process.namespace.processes.borrow();
    let record = records.get(&pid).ok_or(ENOENT)?;
    let ticks = |nanoseconds: u64| nanoseconds / NANOS_PER_TICK;
    let mut tail = Vec::with_capacity(49);
    tail.extend_from_slice(&[
        record.parent as u64,
        record.group as u64,
        record.session as u64,
    ]);
    // tty_nr, tpgid, flags, minflt, cminflt, majflt, cmajflt
    tail.extend_from_slice(&[0; 7]);
    tail.push(ticks(record.usage.cpu_ns));
    tail.push(0);
    tail.push(ticks(record.children.cpu_ns));
    tail.push(0);
    tail.extend_from_slice(&[20, 0, record.threads as u64, 0]);
    tail.extend_from_slice(&[
        ticks(record.start_ns),
        record.vsize,
        record.rss / crate::elf::PAGE_SIZE as u64,
        u64::MAX,
    ]);
    tail.extend_from_slice(&[0; 12]);
    tail.push(17);
    tail.extend_from_slice(&[0; 13]);
    tail.push(record.status.map_or(0, |status| status.encode() as u64));
    let mut numbers = String::new();
    for value in tail {
        numbers.push(' ');
        let _ = write!(numbers, "{value}");
    }
    let mut text = alloc::format!("{pid} (").into_bytes();
    text.extend_from_slice(&command_name(&record.program));
    text.extend_from_slice(b") ");
    text.push(record.state);
    text.extend_from_slice(numbers.as_bytes());
    text.push(b'\n');
    Ok(text)
}

fn meminfo(process: &Process<'_>) -> Result<Vec<u8>, Errno> {
    let (total, free) = process.supervisor.memory_stats().map_err(|_| EIO)?;
    let kilobytes = |bytes: u64| bytes / 1024;
    let mut data = String::new();
    for (name, value) in [
        ("MemTotal", kilobytes(total)),
        ("MemFree", kilobytes(free)),
        // There is no page cache or reclaimable slab backing user data yet.
        ("MemAvailable", kilobytes(free)),
        ("Buffers", 0),
        ("Cached", 0),
        ("SwapCached", 0),
        ("Active", kilobytes(total.saturating_sub(free))),
        ("Inactive", 0),
        ("SwapTotal", 0),
        ("SwapFree", 0),
        ("Shmem", 0),
    ] {
        let _ = writeln!(data, "{name}: {value:>8} kB");
    }
    Ok(data.into_bytes())
}

fn uptime() -> Result<Vec<u8>, Errno> {
    let now = runtime::thread::now_ns().map_err(|_| EIO)?;
    let seconds = now / 1_000_000_000;
    let hundredths = (now % 1_000_000_000) / 10_000_000;
    // The kernel does not account idle CPU time, so the second field stays 0.
    Ok(alloc::format!("{seconds}.{hundredths:02} 0.00\n").into_bytes())
}

fn maps(process: &Process<'_>, smaps: bool) -> Result<Vec<u8>, Errno> {
    let regions = process.address_space.regions().map_err(|_| EIO)?;
    let mut data = Vec::new();
    for region in regions {
        let label: Option<&[u8]> = match region.origin.as_deref() {
            Some(Origin::File(path)) => Some(path),
            Some(Origin::Stack) => Some(b"[stack]"),
            Some(Origin::Heap) => Some(b"[heap]"),
            None => None,
        };
        data.extend_from_slice(
            alloc::format!(
                "{:08x}-{:08x} {} {:08x} 00:00 0",
                region.start,
                region.end,
                permissions(region.permissions),
                region.file_offset,
            )
            .as_bytes(),
        );
        if let Some(label) = label {
            data.push(b' ');
            data.extend_from_slice(label);
        }
        data.push(b'\n');
        if smaps {
            let size = (region.end - region.start) / 1024;
            for (name, value) in [
                ("Size", size),
                ("KernelPageSize", 4),
                ("MMUPageSize", 4),
                ("Rss", size),
                ("Pss", size),
                ("Shared_Clean", 0),
                ("Shared_Dirty", 0),
                ("Private_Clean", 0),
                ("Private_Dirty", size),
                ("Referenced", size),
                ("Anonymous", 0),
                ("Swap", 0),
            ] {
                data.extend_from_slice(alloc::format!("{name}: {value:>16} kB\n").as_bytes());
            }
        }
    }
    Ok(data)
}

fn permissions(permissions: Permissions) -> &'static str {
    match permissions {
        Permissions::NoAccess => "---p",
        Permissions::ReadOnly => "r--p",
        Permissions::ReadWrite => "rw-p",
        Permissions::ExecuteOnly => "--xp",
        Permissions::ReadExecute => "r-xp",
    }
}
