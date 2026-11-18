// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Character device paths exposed by the Linux compatibility layer.

/// Returns the metadata of a device node served without a filesystem entry.
pub(crate) fn metadata(path: &[u8]) -> Option<fs::Metadata> {
    // REFERENCE: https://www.kernel.org/doc/html/latest/admin-guide/devices.html
    let (mode, inode) = match path {
        b"/dev/null" => (0o020666, 3),
        b"/dev/zero" => (0o020666, 5),
        b"/dev/random" => (0o020666, 8),
        b"/dev/urandom" => (0o020666, 9),
        b"/dev/tty" => (0o020666, 0x0500),
        b"/dev/console" => (0o020600, 0x0501),
        _ => return None,
    };
    Some(fs::Metadata {
        device: 0x0103,
        inode,
        links: 1,
        mode,
        ..fs::Metadata::default()
    })
}
