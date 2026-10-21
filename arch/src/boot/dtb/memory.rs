// SPDX-License-Identifier: MPL-2.0

//! 合并固件 RAM 和保留项，生成覆盖全部 RAM 的互斥区域表。

use arrayvec::ArrayVec;
use kernel::{
    MemoryInfo,
    memory::{MemoryRegion, MemoryRegionState, PhysRange},
};

use super::{
    Dtb, Result,
    nodes::{Node, cell_sizes, enabled, property, registers},
    range,
};

const MAX_FIRMWARE_RANGES: usize = 256;
type Regions = ArrayVec<MemoryRegion, MAX_FIRMWARE_RANGES>;

pub(super) fn parse(dtb: &Dtb<'_>, output: &mut MemoryInfo) -> Result<()> {
    let mut source = Regions::new();
    ram(dtb, &mut source)?;
    reserved(dtb, &mut source)?;
    let mut boundaries = ArrayVec::<u64, { 2 * MAX_FIRMWARE_RANGES }>::new();
    boundaries.extend(
        source
            .iter()
            .flat_map(|region| [region.range.start, region.range.end]),
    );
    boundaries.as_mut_slice().sort_unstable();
    output.regions.clear();
    for region in normalize(&source, &boundaries) {
        output
            .regions
            .try_push(region)
            .map_err(|_| "too many normalized memory regions")?;
    }
    Ok(())
}

fn normalize<'a>(
    source: &'a [MemoryRegion],
    boundaries: &'a [u64],
) -> impl Iterator<Item = MemoryRegion> + 'a {
    let mut segments = boundaries
        .windows(2)
        .filter_map(|pair| {
            if pair[0] == pair[1] {
                return None;
            }
            let address = PhysRange {
                start: pair[0],
                end: pair[1],
            };
            let covering = source
                .iter()
                .filter(|region| region.range.contains(address));
            if !covering
                .clone()
                .any(|region| region.state == MemoryRegionState::Available)
            {
                return None;
            }
            let state = covering.map(|region| region.state).max()?;
            Some(MemoryRegion {
                range: address,
                state,
            })
        })
        .peekable();
    core::iter::from_fn(move || {
        let mut region = segments.next()?;
        while let Some(next) = segments
            .next_if(|next| region.range.end == next.range.start && region.state == next.state)
        {
            region.range.end = next.range.end;
        }
        Some(region)
    })
}

fn ram(dtb: &Dtb<'_>, output: &mut Regions) -> Result<()> {
    for node in dtb.root.children()? {
        let node = node?;
        if enabled(node)? && property::<&str>(node, "device_type")? == Some("memory") {
            read_regions(node, MemoryRegionState::Available, output)?;
        }
    }
    if output.is_empty() {
        return Err("FDT contains no usable RAM description".into());
    }
    Ok(())
}

fn reserved(dtb: &Dtb<'_>, output: &mut Regions) -> Result<()> {
    for range in dtb.reservations()? {
        output
            .try_push(MemoryRegion {
                range: range?.page_envelope()?,
                state: MemoryRegionState::Reserved,
            })
            .map_err(|_| "too many firmware memory ranges")?;
    }
    let Some(parent) = dtb.root.child("reserved-memory")? else {
        return Ok(());
    };
    if !enabled(parent)? {
        return Ok(());
    }
    if cell_sizes(parent)? != cell_sizes(dtb.root)? {
        return Err("reserved-memory cell sizes differ from the root".into());
    }
    if !parent
        .raw_property("ranges")?
        .is_some_and(|property| property.value.is_empty())
    {
        return Err("reserved-memory must use the root address space".into());
    }
    for node in parent.children()? {
        let node = node?;
        if !enabled(node)? {
            continue;
        }
        let state = match (
            node.raw_property("no-map")?.is_some(),
            node.raw_property("reusable")?.is_some(),
        ) {
            (true, true) => return Err("reserved memory declares both no-map and reusable".into()),
            (true, false) => MemoryRegionState::NoMap,
            (false, true) => MemoryRegionState::BootReserved,
            (false, false) => MemoryRegionState::Reserved,
        };
        read_regions(node, state, output)?;
    }
    Ok(())
}

fn read_regions(node: Node<'_>, state: MemoryRegionState, output: &mut Regions) -> Result<()> {
    for entry in registers(node)?.iter::<u64, u64>() {
        let entry = entry?;
        let range = range(entry.address, entry.len)?;
        let pages = range.page_envelope()?;
        if state == MemoryRegionState::Available && range != pages {
            return Err("RAM range is not page-aligned".into());
        }
        if state == MemoryRegionState::Available
            && output.iter().any(|other| range.overlaps(other.range))
        {
            return Err("RAM regions overlap".into());
        }
        output
            .try_push(MemoryRegion {
                range: pages,
                state,
            })
            .map_err(|_| "too many firmware memory ranges")?;
    }
    Ok(())
}
