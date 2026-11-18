// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Directory entries and Linux mount-point overlays.

use super::local;
use crate::{Services, process::Process, syscall::abi::Errno};
use alloc::{format, vec::Vec};

#[derive(Clone)]
pub(crate) struct Entry {
    pub name: Vec<u8>,
    pub inode: u64,
    pub kind: u8,
}

impl From<fs::DirEntry> for Entry {
    fn from(entry: fs::DirEntry) -> Self {
        Self {
            name: entry.name,
            inode: entry.inode,
            kind: match entry.kind {
                fs::FileType::Unknown => 0,
                fs::FileType::Directory => 4,
                fs::FileType::Regular => 8,
                fs::FileType::SymbolicLink => 10,
            },
        }
    }
}

pub(crate) fn entries(
    process: &Process<'_>,
    services: &Services<'_>,
    path: &[u8],
) -> Result<Vec<Entry>, Errno> {
    let mut names: Vec<Vec<u8>> = match path {
        b"/dev" => [
            b"null".as_slice(),
            b"zero",
            b"random",
            b"urandom",
            b"tty",
            b"console",
            b"ptmx",
            b"pts",
            b"mqueue",
            b"misc",
            b"rtc",
            b"rtc0",
        ]
        .into_iter()
        .map(<[u8]>::to_vec)
        .collect(),
        b"/dev/misc" => alloc::vec![b"rtc".to_vec()],
        b"/dev/pts" => process
            .namespace
            .ptys
            .live()
            .iter()
            .map(|pair| format!("{}", pair.number()).into_bytes())
            .collect(),
        b"/dev/mqueue" => process
            .namespace
            .mqueues
            .list()
            .iter()
            .map(|queue| queue.name().to_vec())
            .collect(),
        b"/proc" => {
            let mut names = [
                b"self".as_slice(),
                b"thread-self",
                b"net",
                b"meminfo",
                b"uptime",
                b"version",
                b"mounts",
            ]
            .into_iter()
            .map(<[u8]>::to_vec)
            .collect::<Vec<_>>();
            names.extend(
                process
                    .namespace
                    .processes
                    .borrow()
                    .keys()
                    .map(|id| format!("{id}").into_bytes()),
            );
            names
        }
        b"/proc/net" => [
            b"dev".as_slice(),
            b"route",
            b"tcp",
            b"tcp6",
            b"udp",
            b"udp6",
            b"unix",
        ]
        .into_iter()
        .map(<[u8]>::to_vec)
        .collect(),
        b"/sys" => alloc::vec![b"devices".to_vec(), b"kernel".to_vec()],
        b"/sys/devices" => alloc::vec![b"system".to_vec()],
        b"/sys/devices/system" => alloc::vec![b"cpu".to_vec()],
        b"/sys/devices/system/cpu" => alloc::vec![b"online".to_vec()],
        b"/sys/kernel" => alloc::vec![b"osrelease".to_vec()],
        _ => {
            let info = local::metadata(process, services, path)?;
            if info.mode & 0o170000 != 0o040000 {
                return Err(crate::syscall::abi::ENOTDIR);
            }
            let mut names = alloc::vec![b"cmdline".to_vec(), b"stat".to_vec(), b"exe".to_vec()];
            if path.strip_prefix(b"/proc/").and_then(local::pid) == Some(process.pid) {
                names.extend([b"status".to_vec(), b"maps".to_vec(), b"smaps".to_vec()]);
            }
            names
        }
    };
    if path == b"/dev"
        && let Some(blocks) = services.blocks.as_ref()
    {
        names.extend(blocks.nodes().iter().map(|node| node.name.clone()));
    }
    names.sort_unstable();
    let mut entries = Vec::new();
    for name in names {
        let mut full = path.to_vec();
        full.push(b'/');
        full.extend_from_slice(&name);
        let info = local::metadata(process, services, &full)?;
        entries.push(Entry {
            name,
            inode: info.inode,
            kind: (info.mode >> 12) as u8,
        });
    }
    let info = local::metadata(process, services, path)?;
    let parent = path
        .iter()
        .rposition(|byte| *byte == b'/')
        .unwrap_or(0)
        .max(1);
    let parent_inode = if parent == 1 {
        0
    } else {
        local::metadata(process, services, &path[..parent])?.inode
    };
    entries.insert(
        0,
        Entry {
            name: b"..".to_vec(),
            inode: parent_inode,
            kind: 4,
        },
    );
    entries.insert(
        0,
        Entry {
            name: b".".to_vec(),
            inode: info.inode,
            kind: 4,
        },
    );
    Ok(entries)
}

pub(crate) fn overlay(entries: &mut Vec<Entry>, name: &[u8], info: fs::Metadata) {
    entries.retain(|entry| entry.name != name);
    entries.push(Entry {
        name: name.to_vec(),
        inode: info.inode,
        kind: (info.mode >> 12) as u8,
    });
}
