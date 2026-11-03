// SPDX-License-Identifier: MPL-2.0

use super::{half, volume::Volume, word};
use crate::{Error, block::BLOCK_SIZE};
use alloc::{collections::BTreeSet, vec::Vec};

#[derive(Clone)]
pub(super) struct Extent {
    pub logical: u32,
    pub physical: u32,
    pub length: u32,
    pub unwritten: bool,
}

impl Volume {
    // REFERENCE: https://www.kernel.org/doc/html/v6.12/filesystems/ext4/ifork.html
    pub fn extents(&mut self, root: &[u8]) -> Result<Vec<Extent>, Error> {
        let mut result = Vec::new();
        let mut visited = BTreeSet::new();
        let depth = half(root, 6);
        if depth > 5 {
            return Err(Error::InvalidData);
        }
        self.extent_node(root, depth, 0, 1u64 << 32, &mut visited, &mut result)?;
        let mut physical: Vec<_> = result
            .iter()
            .map(|extent| (extent.physical, extent.physical + extent.length))
            .collect();
        physical.sort_unstable();
        if physical.windows(2).any(|pair| pair[0].1 > pair[1].0) {
            return Err(Error::InvalidData);
        }
        if visited.iter().any(|block| {
            physical
                .iter()
                .any(|(start, end)| (*start..*end).contains(block))
        }) {
            return Err(Error::InvalidData);
        }
        Ok(result)
    }

    pub fn extent_blocks(&mut self, root: &[u8]) -> Result<Vec<u32>, Error> {
        let mut result = Vec::new();
        let mut pending = alloc::vec![(root.to_vec(), half(root, 6))];
        let mut visited = BTreeSet::new();
        while let Some((bytes, depth)) = pending.pop() {
            if depth == 0 {
                continue;
            }
            for index in 0..usize::from(half(&bytes, 2)) {
                let number = word(&bytes, 16 + 12 * index);
                if !visited.insert(number) {
                    return Err(Error::InvalidData);
                }
                let child = self.block(number)?;
                pending.push((child.to_vec(), depth - 1));
                result.push(number);
            }
        }
        Ok(result)
    }

    fn extent_node(
        &mut self,
        bytes: &[u8],
        depth: u16,
        first: u64,
        last: u64,
        visited: &mut BTreeSet<u32>,
        result: &mut Vec<Extent>,
    ) -> Result<(), Error> {
        let entries = usize::from(half(bytes, 2));
        let maximum = usize::from(half(bytes, 4));
        if half(bytes, 0) != 0xf30a
            || half(bytes, 6) != depth
            || maximum == 0
            || maximum > (bytes.len() - 12) / 12
            || entries > maximum
            || depth != 0 && entries == 0
        {
            return Err(Error::InvalidData);
        }
        let mut end = first;
        for index in 0..entries {
            let offset = 12 + index * 12;
            let logical = word(bytes, offset);
            if u64::from(logical) < end || u64::from(logical) >= last {
                return Err(Error::InvalidData);
            }
            if depth == 0 {
                let raw = half(bytes, offset + 4);
                let length = u32::from(if raw > 32768 { raw - 32768 } else { raw });
                let physical = word(bytes, offset + 8);
                end = u64::from(logical) + u64::from(length);
                if length == 0
                    || half(bytes, offset + 6) != 0
                    || end > last
                    || end > 524288
                    || physical
                        .checked_add(length)
                        .is_none_or(|limit| limit > self.blocks)
                {
                    return Err(Error::InvalidData);
                }
                for block in physical..physical + length {
                    if !self.allocated_block(block)? {
                        return Err(Error::InvalidData);
                    }
                }
                result.push(Extent {
                    logical,
                    physical,
                    length,
                    unwritten: raw > 32768,
                });
            } else {
                let next = if index + 1 < entries {
                    u64::from(word(bytes, offset + 12))
                } else {
                    last
                };
                let block = word(bytes, offset + 4);
                if half(bytes, offset + 8) != 0
                    || next <= u64::from(logical)
                    || next > last
                    || !self.allocated_block(block)?
                    || !visited.insert(block)
                    || visited.len() as u32 > self.blocks
                {
                    return Err(Error::InvalidData);
                }
                let child = self.block(block)?;
                self.extent_node(
                    &child[..BLOCK_SIZE],
                    depth - 1,
                    logical.into(),
                    next,
                    visited,
                    result,
                )?;
                end = next;
            }
        }
        Ok(())
    }
}
