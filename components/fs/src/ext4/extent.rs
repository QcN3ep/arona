// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::{half, inode::Inode, volume::Volume, word};
use crate::Error;
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
    pub fn extent_blocks(&mut self, inode: &Inode) -> Result<Vec<u32>, Error> {
        let mut result = Vec::new();
        let mut pending = alloc::vec![(inode.root.to_vec(), half(&inode.root, 6))];
        let mut visited = BTreeSet::new();
        let seed = self.checksum.inode_seed(inode.number, inode.generation);
        while let Some((bytes, depth)) = pending.pop() {
            let count = usize::from(half(&bytes, 2));
            if half(&bytes, 0) != 0xf30a || depth > 5 || count > (bytes.len() - 12) / 12 {
                return Err(Error::InvalidData);
            }
            if depth == 0 {
                continue;
            }
            for index in 0..count {
                let number = word(&bytes, 16 + 12 * index);
                if !self.allocated_block(number)? || !visited.insert(number) {
                    return Err(Error::InvalidData);
                }
                let child = self.block(number)?;
                self.checksum.extent(seed, &child[..self.block_size])?;
                pending.push((child[..self.block_size].to_vec(), depth - 1));
                result.push(number);
            }
        }
        Ok(result)
    }

    pub fn extents(&mut self, inode: &Inode) -> Result<Vec<Extent>, Error> {
        let root = &inode.root;
        let mut result = Vec::new();
        let mut visited = BTreeSet::new();
        let depth = half(root, 6);
        if depth > 5 {
            return Err(Error::InvalidData);
        }
        let seed = self.checksum.inode_seed(inode.number, inode.generation);
        self.extent_node(root, depth, 0..1u64 << 32, seed, &mut visited, &mut result)?;
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

    fn extent_node(
        &mut self,
        bytes: &[u8],
        depth: u16,
        bounds: core::ops::Range<u64>,
        seed: u32,
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
        let mut end = bounds.start;
        for index in 0..entries {
            let offset = 12 + index * 12;
            let logical = word(bytes, offset);
            if u64::from(logical) < end || u64::from(logical) >= bounds.end {
                return Err(Error::InvalidData);
            }
            if depth == 0 {
                let raw = half(bytes, offset + 4);
                let length = u32::from(if raw > 32768 { raw - 32768 } else { raw });
                let physical = word(bytes, offset + 8);
                end = u64::from(logical) + u64::from(length);
                if length == 0
                    || half(bytes, offset + 6) != 0
                    || end > bounds.end
                    || end > 2 * 1024 * 1024 * 1024 / self.block_size as u64
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
                    bounds.end
                };
                let block = word(bytes, offset + 4);
                if half(bytes, offset + 8) != 0
                    || next <= u64::from(logical)
                    || next > bounds.end
                    || !self.allocated_block(block)?
                    || !visited.insert(block)
                    || visited.len() as u32 > self.blocks
                {
                    return Err(Error::InvalidData);
                }
                let child = self.block(block)?;
                self.checksum.extent(seed, &child[..self.block_size])?;
                self.extent_node(
                    &child[..self.block_size],
                    depth - 1,
                    u64::from(logical)..next,
                    seed,
                    visited,
                    result,
                )?;
                end = next;
            }
        }
        Ok(())
    }
}
