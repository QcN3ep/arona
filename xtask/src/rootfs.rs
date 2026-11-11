// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! CPIO packaging of an external root directory.

use super::append_entry;
use std::{fs, os::unix::fs::PermissionsExt, path::Path};

pub(super) fn append(
    bytes: &mut Vec<u8>,
    root: Option<&Path>,
    reserved: &[&str],
) -> Result<(), String> {
    if let Some(root) = root {
        if !root.is_dir() {
            return Err(format!("rootfs is not a directory: {}", root.display()));
        }
        directory(bytes, root, root, reserved)?;
    }
    Ok(())
}

fn directory(
    bytes: &mut Vec<u8>,
    root: &Path,
    path: &Path,
    reserved: &[&str],
) -> Result<(), String> {
    let mut entries = fs::read_dir(path)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let name = path
            .strip_prefix(root)
            .map_err(|error| error.to_string())?
            .to_str()
            .ok_or("rootfs path must be valid UTF-8")?;
        if name == "TRAILER!!!" || reserved.contains(&name) {
            return Err(format!("reserved initramfs path: {name}"));
        }
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if !metadata.is_dir()
            && reserved.iter().any(|path| {
                path.strip_prefix(name)
                    .is_some_and(|tail| tail.starts_with('/'))
            })
        {
            return Err(format!("reserved initramfs directory: {name}"));
        }
        let permissions = metadata.permissions().mode() & 0o7777;
        if metadata.is_dir() {
            append_entry(bytes, name, 0o040000 | permissions, &[])?;
            directory(bytes, root, &path, reserved)?;
        } else if metadata.is_file() {
            let data = fs::read(&path).map_err(|error| error.to_string())?;
            append_entry(bytes, name, 0o100000 | permissions, &data)?;
        } else if metadata.file_type().is_symlink() {
            let target = fs::read_link(&path).map_err(|error| error.to_string())?;
            let target = target
                .to_str()
                .ok_or("rootfs symlink must be valid UTF-8")?;
            append_entry(bytes, name, 0o120000 | permissions, target.as_bytes())?;
        } else {
            return Err(format!("unsupported rootfs entry: {}", path.display()));
        }
    }
    Ok(())
}
