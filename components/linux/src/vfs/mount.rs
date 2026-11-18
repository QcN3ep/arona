// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! The Linux mount view over native volumes and local filesystem providers.

use alloc::vec::Vec;

#[derive(Clone, Copy)]
pub(crate) enum Backend {
    Native,
    Proc,
    Sys,
    Device,
    Pty,
    Mqueue,
}

pub(crate) struct Mount {
    pub path: &'static [u8],
    pub backend: Backend,
    pub name: &'static str,
    pub magic: u64,
    pub readonly: bool,
}

pub(crate) const MOUNTS: &[Mount] = &[
    Mount {
        path: b"/",
        backend: Backend::Native,
        name: "",
        magic: 0,
        readonly: false,
    },
    Mount {
        path: b"/dev",
        backend: Backend::Device,
        name: "devfs",
        magic: 0x1373,
        readonly: false,
    },
    Mount {
        path: b"/proc",
        backend: Backend::Proc,
        name: "proc",
        magic: 0x9fa0,
        readonly: true,
    },
    Mount {
        path: b"/sys",
        backend: Backend::Sys,
        name: "sysfs",
        magic: 0x6265_6572,
        readonly: true,
    },
    Mount {
        path: b"/dev/pts",
        backend: Backend::Pty,
        name: "devpts",
        magic: 0x1cd1,
        readonly: false,
    },
    Mount {
        path: b"/dev/mqueue",
        backend: Backend::Mqueue,
        name: "mqueue",
        magic: 0x1980_0202,
        readonly: false,
    },
    Mount {
        path: b"/dev/shm",
        backend: Backend::Native,
        name: "",
        magic: 0,
        readonly: false,
    },
    Mount {
        path: b"/dev/hugepages",
        backend: Backend::Native,
        name: "",
        magic: 0,
        readonly: false,
    },
];

pub(crate) fn at(path: &[u8]) -> &'static Mount {
    MOUNTS
        .iter()
        .filter(|mount| {
            mount.path == b"/"
                || path == mount.path
                || path
                    .strip_prefix(mount.path)
                    .is_some_and(|tail| tail.starts_with(b"/"))
        })
        .max_by_key(|mount| mount.path.len())
        .unwrap()
}

pub(crate) fn statistics(path: &[u8]) -> fs::Statistics {
    let mount = at(path);
    fs::Statistics {
        kind: mount.magic,
        device: identity(path),
        block_size: 4096,
        name_length: 255,
        flags: u64::from(mount.readonly),
        ..fs::Statistics::default()
    }
}

pub(crate) fn identity(path: &[u8]) -> u64 {
    0x8000_0000_0000_0000 | at(path).magic
}

pub(crate) fn line(output: &mut Vec<u8>, source: &[u8], path: &[u8], kind: &str, readonly: bool) {
    for field in [source, path] {
        for &byte in field {
            match byte {
                b' ' => output.extend_from_slice(b"\\040"),
                b'\t' => output.extend_from_slice(b"\\011"),
                b'\n' => output.extend_from_slice(b"\\012"),
                b'\\' => output.extend_from_slice(b"\\134"),
                _ => output.push(byte),
            }
        }
        output.push(b' ');
    }
    output.extend_from_slice(kind.as_bytes());
    output.extend_from_slice(if readonly {
        b" ro,relatime 0 0\n"
    } else {
        b" rw,relatime 0 0\n"
    });
}

pub(crate) fn append_volume(output: &mut Vec<u8>, volume: &fs::Volume) {
    if !matches!(at(&volume.path).backend, Backend::Native) {
        return;
    }
    let kind = match volume.statistics.kind {
        0xef53 => "ext4",
        0x4d44 => "fat32",
        0x0102_1994 => "tmpfs",
        0x9584_58f6 => "hugetlbfs",
        0x8584_58f6 => "ramfs",
        _ => "archive",
    };
    line(
        output,
        kind.as_bytes(),
        &volume.path,
        kind,
        volume.statistics.flags & 1 != 0,
    );
}

pub(crate) fn split(path: &[u8]) -> (&[u8], &[u8]) {
    let end = path.iter().rposition(|byte| *byte == b'/').unwrap_or(0);
    (&path[..end.max(1)], &path[end + 1..])
}
