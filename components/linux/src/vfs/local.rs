// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Nodes supplied by the Linux process and device registries.

use crate::{
    Services,
    process::Process,
    syscall::abi::{ENOENT, Errno},
};
use alloc::{format, vec::Vec};

const DIRECTORIES: &[&[u8]] = &[
    b"/dev",
    b"/dev/misc",
    b"/dev/pts",
    b"/dev/mqueue",
    b"/proc",
    b"/proc/net",
    b"/sys",
    b"/sys/devices",
    b"/sys/devices/system",
    b"/sys/devices/system/cpu",
    b"/sys/kernel",
];

pub(crate) fn contains(path: &[u8]) -> bool {
    !matches!(
        super::mount::at(path).backend,
        super::mount::Backend::Native
    )
}

pub(crate) fn link(process: &Process<'_>, path: &[u8]) -> Option<Vec<u8>> {
    match path {
        b"/proc/self" | b"/proc/thread-self" => Some(format!("{}", process.pid).into_bytes()),
        _ => super::procfs::link(process, path),
    }
}

pub(crate) fn metadata(
    process: &Process<'_>,
    services: &Services<'_>,
    path: &[u8],
) -> Result<fs::Metadata, Errno> {
    if let Some(index) = DIRECTORIES.iter().position(|directory| *directory == path) {
        let inode = if path == b"/dev/mqueue" {
            crate::syscall::mqueue::path_metadata(process, path)
                .unwrap()?
                .inode
        } else {
            0x8000_0000 + index as u64
        };
        return Ok(directory_metadata(path, inode));
    }
    if let Some(id) = path.strip_prefix(b"/proc/").and_then(pid) {
        if process.namespace.processes.borrow().contains_key(&id) {
            return Ok(fs::Metadata {
                device: device_id(path),
                mount: super::mount::identity(path),
                ..directory(super::procfs::process_inode(id, 0))
            });
        }
        return Err(ENOENT);
    }
    if let Some(target) = link(process, path) {
        return Ok(fs::Metadata {
            inode: identity(path),
            mode: 0o120777,
            size: target.len() as u64,
            links: 1,
            device: device_id(path),
            mount: super::mount::identity(path),
            ..fs::Metadata::default()
        });
    }
    if let Some(file) = super::procfs::resolve(process, path) {
        return Ok(fs::Metadata {
            inode: file.inode(process.pid),
            mode: 0o100444,
            links: 1,
            device: device_id(path),
            mount: super::mount::identity(path),
            ..fs::Metadata::default()
        });
    }
    if let Some(info) = crate::syscall::mqueue::path_metadata(process, path) {
        return info.map(|info| fs::Metadata {
            device: device_id(path),
            mount: super::mount::identity(path),
            ..info
        });
    }
    if let Some(mut info) = super::devfs::metadata(path) {
        info.rdev = if info.inode < 256 {
            1 << 8 | info.inode
        } else {
            info.inode
        };
        info.device = device_id(path);
        info.mount = super::mount::identity(path);
        return Ok(info);
    }
    let special = match path {
        b"/dev/rtc" | b"/dev/rtc0" | b"/dev/misc/rtc" => Some((0x11, 254 << 8, 0o020600)),
        b"/dev/ptmx" => Some((0x0502, 5 << 8 | 2, 0o020666)),
        _ => path
            .strip_prefix(b"/dev/pts/")
            .and_then(pid)
            .and_then(|id| {
                process
                    .namespace
                    .ptys
                    .slave(u32::try_from(id).ok()?)
                    .map(|_| (0x8800_0000 + id as u64, 136 << 8 | id as u64, 0o020620))
            }),
    };
    if let Some((inode, rdev, mode)) = special {
        return Ok(fs::Metadata {
            inode,
            rdev,
            mode,
            links: 1,
            device: device_id(path),
            mount: super::mount::identity(path),
            ..fs::Metadata::default()
        });
    }
    if let Some(node) = path.strip_prefix(b"/dev/").and_then(|name| {
        services
            .blocks
            .as_ref()?
            .nodes()
            .iter()
            .find(|node| node.name == name)
    }) {
        let minor = node.disk as u64 * 16 + u64::from(node.partition);
        return Ok(fs::Metadata {
            inode: 0x1000 + minor,
            rdev: 254 << 8 | minor,
            mode: 0o060660,
            links: 1,
            device: device_id(path),
            mount: super::mount::identity(path),
            ..fs::Metadata::default()
        });
    }
    Err(ENOENT)
}

pub(crate) fn device_id(path: &[u8]) -> u64 {
    super::mount::identity(path)
}

pub(crate) fn directory_metadata(path: &[u8], inode: u64) -> fs::Metadata {
    fs::Metadata {
        inode,
        device: device_id(path),
        mount: super::mount::identity(path),
        mode: if path == b"/dev/mqueue" {
            0o041777
        } else {
            0o040555
        },
        links: 2,
        ..fs::Metadata::default()
    }
}

fn directory(inode: u64) -> fs::Metadata {
    fs::Metadata {
        inode,
        mode: 0o040555,
        links: 2,
        ..fs::Metadata::default()
    }
}

fn identity(path: &[u8]) -> u64 {
    path.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
    })
}

pub(crate) fn pid(bytes: &[u8]) -> Option<usize> {
    if bytes.is_empty() || bytes.len() > 1 && bytes[0] == b'0' {
        return None;
    }
    bytes.iter().try_fold(0usize, |value, byte| {
        byte.is_ascii_digit().then_some(())?;
        value
            .checked_mul(10)?
            .checked_add(usize::from(*byte - b'0'))
    })
}
