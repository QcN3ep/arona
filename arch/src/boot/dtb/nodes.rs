// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Traverse enabled nodes, preserving the current ancestor path and completing bus address translation.

use arrayvec::ArrayVec;
use fdt::{
    helpers::UnalignedFallibleNode,
    nodes::{NodeChildrenIter, NodeName},
    parsing::{NoPanic, unaligned::UnalignedParser},
    properties::{Status, cells::CellSizes, ranges::Ranges, reg::Reg, values::PropertyValue},
};
use kernel::memory::PhysRange;

use super::{Result, range};

pub(super) type Node<'a> = UnalignedFallibleNode<'a>;
const MAX_DEPTH: usize = 32;
type Children<'a> = NodeChildrenIter<'a, (UnalignedParser<'a>, NoPanic)>;

struct Frame<'a> {
    node: Node<'a>,
    name: NodeName<'a>,
    children: Children<'a>,
}

impl<'a> Frame<'a> {
    fn new(node: Node<'a>) -> Result<Self> {
        Ok(Self {
            node,
            name: node.name()?,
            children: node.children()?.iter(),
        })
    }
}

pub(super) struct Nodes<'a> {
    path: ArrayVec<Frame<'a>, { MAX_DEPTH + 1 }>,
    root: Option<Node<'a>>,
}

impl<'a> Nodes<'a> {
    pub fn new(root: Node<'a>) -> Self {
        Self {
            path: const { ArrayVec::new_const() },
            root: Some(root),
        }
    }

    pub fn next(&mut self) -> Result<Option<Node<'a>>> {
        if let Some(root) = self.root.take() {
            if enabled(root)? {
                self.path.push(Frame::new(root)?);
                return Ok(Some(root));
            }
            return Ok(None);
        }
        while let Some(frame) = self.path.last_mut() {
            let Some(child) = frame.children.next() else {
                self.path.pop();
                continue;
            };
            let child = child?;
            if enabled(child)? {
                self.path
                    .try_push(Frame::new(child)?)
                    .map_err(|_| "FDT depth exceeds 32")?;
                return Ok(Some(child));
            }
        }
        Ok(None)
    }

    pub fn node(&self) -> Node<'a> {
        self.path.last().unwrap().node
    }

    pub fn ancestors(&self) -> impl Iterator<Item = Node<'a>> + Clone + '_ {
        self.path.iter().rev().skip(1).map(|frame| frame.node)
    }

    pub fn path(&self) -> impl Iterator<Item = u8> + '_ {
        self.path
            .iter()
            .skip(1)
            .flat_map(|frame| core::iter::once(b'/').chain(name_bytes(frame.name)))
    }

    pub fn translate(&self, mut address: PhysRange) -> Result<PhysRange> {
        for bus in self.ancestors().take(self.path.len().saturating_sub(2)) {
            let Some(ranges) = ranges(bus)? else {
                continue;
            };
            let mut mapped = None;
            for entry in ranges.iter::<u64, u64, u64>() {
                let entry = entry?;
                if range(entry.child_bus_address, entry.len)?.contains(address) {
                    if mapped.is_some() {
                        return Err("bus address mapping is ambiguous".into());
                    }
                    let start = entry
                        .parent_bus_address
                        .checked_add(address.start - entry.child_bus_address)
                        .ok_or("bus address translation overflow")?;
                    mapped = Some(range(start, address.end - address.start)?);
                }
            }
            address = mapped.ok_or("reg is not covered by bus ranges")?;
        }
        Ok(address)
    }
}

fn name_bytes(name: NodeName<'_>) -> impl Iterator<Item = u8> + '_ {
    name.name.bytes().chain(
        name.unit_address
            .into_iter()
            .flat_map(|address| core::iter::once(b'@').chain(address.bytes())),
    )
}

pub(super) fn enabled(node: Node<'_>) -> Result<bool> {
    Ok(node
        .property::<Status>()?
        .is_none_or(|status| status.is_okay()))
}

pub(super) fn property<'a, T: PropertyValue<'a>>(node: Node<'a>, name: &str) -> Result<Option<T>> {
    node.raw_property(name)?
        .map(|property| {
            property
                .as_value()
                .map_err(|error| fdt::FdtError::from(error).into())
        })
        .transpose()
}

pub(super) fn registers(node: Node<'_>) -> Result<Reg<'_>> {
    let parent = node.parent().ok_or("reg node is missing its parent")?;
    let cells = cell_sizes(parent)?;
    if cells.address_cells == 0 && cells.size_cells == 0 {
        return Err("reg cell sizes are both zero".into());
    }
    let reg = node.reg()?.ok_or("node is missing reg")?;
    if reg.iter_raw().next().is_none() {
        return Err("reg has no entries".into());
    }
    Ok(reg)
}

pub(super) fn cell_sizes(node: Node<'_>) -> Result<CellSizes> {
    let address = property::<u32>(node, "#address-cells")?;
    let size = property::<u32>(node, "#size-cells")?;
    let cells = CellSizes {
        address_cells: address.unwrap_or(2) as usize,
        size_cells: size.unwrap_or(1) as usize,
    };
    // fdt 0.2 uses a whole set of defaults when either declaration is missing.
    if (address.is_none() || size.is_none()) && cells != CellSizes::default() {
        return Err("partial non-default cell size declarations are not supported".into());
    }
    Ok(cells)
}

/// A missing ranges property means the address is unreachable; empty ranges means an identity mapping and returns None.
pub(super) fn ranges(node: Node<'_>) -> Result<Option<Ranges<'_>>> {
    let raw = node
        .raw_property("ranges")?
        .ok_or("bus is missing ranges")?;
    if raw.value.is_empty() {
        return Ok(None);
    }
    let child = cell_sizes(node)?;
    let parent = cell_sizes(node.parent().ok_or("bus is missing its parent")?)?;
    let stride = 4 * (child.address_cells + parent.address_cells + child.size_cells);
    if stride == 0 || !raw.value.len().is_multiple_of(stride) {
        return Err("ranges contains a partial entry".into());
    }
    Ok(node.ranges()?)
}

pub(super) fn integer(node: Node<'_>, name: &str) -> Result<Option<u64>> {
    node.raw_property(name)?
        .map(|property| match property.value.len() {
            4 => Ok(u64::from(u32::from_be_bytes(
                property.value.try_into().unwrap(),
            ))),
            8 => Ok(u64::from_be_bytes(property.value.try_into().unwrap())),
            _ => Err("integer property must contain 32 or 64 bits".into()),
        })
        .transpose()
}
