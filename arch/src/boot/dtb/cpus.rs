// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Build a processor list sorted by hardware ID from enabled CPU nodes.

use arrayvec::ArrayVec;
use kernel::{CpuInfo, boot::MAX_CPUS};

use super::{
    Dtb, Result,
    nodes::{enabled, integer, property, registers},
};

pub(super) fn parse(
    dtb: &Dtb<'_>,
    output: &mut ArrayVec<CpuInfo, MAX_CPUS>,
) -> Result<Option<u64>> {
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
        let mut entries = registers(node)?.iter::<u64, u64>();
        let hardware_id = entries.next().ok_or("CPU has no hardware ID")??.address;
        if entries.next().is_some() {
            return Err("CPU has multiple hardware IDs".into());
        }
        output
            .try_push(CpuInfo { hardware_id })
            .map_err(|_| "too many CPUs")?;
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
    let frequency = integer(parent, "timebase-frequency")?;
    if frequency == Some(0) {
        return Err("timebase frequency is zero".into());
    }
    Ok(frequency)
}
