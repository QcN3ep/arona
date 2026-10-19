// SPDX-License-Identifier: MPL-2.0

//! 从已启用的 CPU 节点生成按硬件 ID 排序的处理器清单。

use arrayvec::ArrayVec;
use kernel::{CpuInfo, boot::MAX_CPUS};

use super::{
    Dtb, Result,
    nodes::{enabled, property, registers},
};

pub(super) fn parse(dtb: &Dtb<'_>, output: &mut ArrayVec<CpuInfo, MAX_CPUS>) -> Result<()> {
    output.clear();
    let parent = dtb.root.child("cpus")?.ok_or("FDT is missing cpus")?;
    if !enabled(parent)? {
        return Err("FDT contains no enabled CPUs".into());
    }
    match (
        property::<u32>(parent, "#address-cells")?,
        property::<u32>(parent, "#size-cells")?,
    ) {
        (Some(1 | 2), Some(0)) => {}
        _ => return Err("unsupported CPU cell sizes".into()),
    }
    for node in parent.children()? {
        let node = node?;
        if !enabled(node)? || property::<&str>(node, "device_type")? != Some("cpu") {
            continue;
        }
        for entry in registers(node)?.iter::<u64, u64>() {
            output
                .try_push(CpuInfo {
                    hardware_id: entry?.address,
                })
                .map_err(|_| "too many CPUs")?;
        }
    }
    if output.is_empty() {
        return Err("FDT contains no enabled CPUs".into());
    }
    let cpus = output.as_mut_slice();
    cpus.sort_unstable_by_key(|cpu| cpu.hardware_id);
    if cpus
        .windows(2)
        .any(|pair| pair[0].hardware_id == pair[1].hardware_id)
    {
        return Err("duplicate CPU hardware ID".into());
    }
    Ok(())
}
