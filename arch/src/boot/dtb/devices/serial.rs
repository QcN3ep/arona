// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Resolve the firmware console and retain its driver options.

use super::super::{Dtb, Result, nodes::property};
use super::{DeviceNode, instance_id};
use kernel::ConsoleInfo;

pub(super) fn console(dtb: &Dtb<'_>, nodes: &[DeviceNode<'_>]) -> Result<Option<ConsoleInfo>> {
    let Some(chosen) = dtb.root.child("chosen")? else {
        return Ok(None);
    };
    let Some(stdout) = property::<&str>(chosen, "stdout-path")? else {
        return Ok(None);
    };
    let (path, options) = stdout
        .split_once(':')
        .map_or((stdout, None), |(path, options)| (path, Some(options)));
    let path = if path.starts_with('/') {
        path
    } else {
        let aliases = dtb
            .root
            .child("aliases")?
            .ok_or("stdout alias has no aliases node")?;
        property::<&str>(aliases, path)?.ok_or("stdout alias is missing")?
    };
    let mut walk = dtb.nodes();
    while let Some(node) = walk.next()? {
        if walk.path().eq(path.bytes()) {
            let name = node.name()?.name;
            let index = nodes
                .iter()
                .position(|device| core::ptr::eq(device.name, name))
                .ok_or("stdout device is unsupported")?;
            return Ok(Some(ConsoleInfo {
                device: instance_id(index),
                options: options
                    .unwrap_or_default()
                    .try_into()
                    .map_err(|_| "stdout options are too long")?,
            }));
        }
    }
    Err("stdout does not reference an enabled device".into())
}
