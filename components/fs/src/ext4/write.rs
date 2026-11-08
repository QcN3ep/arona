// SPDX-License-Identifier: MPL-2.0

//! 文件内容、长度与 extent 树更新。

use super::{extent::Extent, inode::Inode, volume::Volume, word};
use crate::{Error, block::BLOCK_SIZE};
use alloc::{rc::Rc, vec::Vec};

impl Volume {
    pub fn write(&mut self, number: u32, offset: usize, input: &[u8]) -> Result<usize, Error> {
        let end = offset
            .checked_add(input.len())
            .filter(|end| *end <= 2 * 1024 * 1024 * 1024)
            .ok_or(Error::FileTooLarge)?;
        let inode = self.inode(number)?;
        if inode.directory() {
            return Err(Error::IsDirectory);
        }
        if inode.symlink() {
            return Err(Error::InvalidRequest);
        }
        if input.is_empty() {
            return Ok(0);
        }
        let extents = self.extents(&inode.root)?;
        self.check_space(&extents, offset, end)?;
        let mut length = input.len();
        loop {
            match self.write_blocks(&inode, offset, &input[..length], &extents) {
                Err(Error::NoSpace) if length > BLOCK_SIZE => length /= 2,
                result => return result,
            }
        }
    }

    fn write_blocks(
        &mut self,
        inode: &Inode,
        offset: usize,
        input: &[u8],
        extents: &[Extent],
    ) -> Result<usize, Error> {
        self.transaction(|volume| {
            let mut inode = inode.clone();
            let mut extents = extents.to_vec();
            if offset as u64 > inode.size {
                volume.zero_tail(&inode, offset)?;
            }
            let mut done = 0;
            while done < input.len() {
                let position = offset + done;
                let logical = (position / BLOCK_SIZE) as u32;
                let inside = position % BLOCK_SIZE;
                let length = (BLOCK_SIZE - inside).min(input.len() - done);
                let existing = extents.iter().position(|extent| {
                    (extent.logical..extent.logical + extent.length).contains(&logical)
                });
                let physical = if let Some(index) = existing {
                    let extent = extents[index].clone();
                    let physical = extent.physical + logical - extent.logical;
                    if extent.unwritten {
                        extents.remove(index);
                        if logical > extent.logical {
                            extents.push(Extent {
                                length: logical - extent.logical,
                                ..extent.clone()
                            });
                        }
                        let remaining = extent.logical + extent.length - logical - 1;
                        if remaining != 0 {
                            extents.push(Extent {
                                logical: logical + 1,
                                physical: physical + 1,
                                length: remaining,
                                ..extent
                            });
                        }
                        extents.push(Extent {
                            logical,
                            physical,
                            length: 1,
                            unwritten: false,
                        });
                        volume.stage(physical, Rc::new([0; BLOCK_SIZE]));
                    }
                    physical
                } else {
                    let physical = volume.allocate_block()?;
                    extents.push(Extent {
                        logical,
                        physical,
                        length: 1,
                        unwritten: false,
                    });
                    physical
                };
                volume.edit(physical, |bytes| {
                    bytes[inside..inside + length].copy_from_slice(&input[done..done + length]);
                    Ok(())
                })?;
                done += length;
            }
            inode.size = inode.size.max((offset + input.len()) as u64);
            volume.replace_extents(&mut inode, extents)?;
            volume.store_inode(&inode)?;
            Ok(input.len())
        })
    }

    fn check_space(&mut self, extents: &[Extent], offset: usize, end: usize) -> Result<(), Error> {
        let first = (offset / BLOCK_SIZE) as u32;
        let last = end.div_ceil(BLOCK_SIZE) as u32;
        let allocated: u32 = extents
            .iter()
            .map(|extent| {
                (extent.logical + extent.length)
                    .min(last)
                    .saturating_sub(extent.logical.max(first))
            })
            .sum();
        let free = word(&self.block(0)?[..], 1024 + 12);
        if last - first - allocated > free {
            return Err(Error::NoSpace);
        }
        Ok(())
    }

    fn zero_tail(&mut self, inode: &Inode, end: usize) -> Result<(), Error> {
        let start = inode.size as usize;
        if start.is_multiple_of(BLOCK_SIZE) {
            return Ok(());
        }
        let logical = (start / BLOCK_SIZE) as u32;
        if let Some(extent) = self.extents(&inode.root)?.into_iter().find(|extent| {
            !extent.unwritten && (extent.logical..extent.logical + extent.length).contains(&logical)
        }) {
            self.edit(extent.physical + logical - extent.logical, |bytes| {
                bytes[start % BLOCK_SIZE..(end - logical as usize * BLOCK_SIZE).min(BLOCK_SIZE)]
                    .fill(0);
                Ok(())
            })?;
        }
        Ok(())
    }

    pub fn truncate(&mut self, number: u32, size: usize) -> Result<(), Error> {
        if size > 2 * 1024 * 1024 * 1024 {
            return Err(Error::FileTooLarge);
        }
        let mut inode = self.inode(number)?;
        if inode.directory() {
            return Err(Error::IsDirectory);
        }
        if inode.symlink() {
            return Err(Error::InvalidRequest);
        }
        self.transaction(|volume| volume.resize(&mut inode, size))
    }

    pub fn resize(&mut self, inode: &mut Inode, size: usize) -> Result<(), Error> {
        let mut extents = self.extents(&inode.root)?;
        let keep = size.div_ceil(BLOCK_SIZE) as u32;
        if (size as u64) < inode.size {
            for extent in &mut extents {
                let remaining = keep.saturating_sub(extent.logical).min(extent.length);
                for block in extent.physical + remaining..extent.physical + extent.length {
                    self.free_block(block)?;
                }
                extent.length = remaining;
            }
            extents.retain(|extent| extent.length != 0);
            if !size.is_multiple_of(BLOCK_SIZE)
                && let Some(extent) = extents.iter().find(|extent| {
                    !extent.unwritten
                        && (extent.logical..extent.logical + extent.length).contains(&(keep - 1))
                })
            {
                self.edit(extent.physical + keep - 1 - extent.logical, |bytes| {
                    bytes[size % BLOCK_SIZE..].fill(0);
                    Ok(())
                })?;
            }
        }
        if size as u64 > inode.size {
            self.zero_tail(inode, size)?;
        }
        inode.size = size as u64;
        self.replace_extents(inode, extents)?;
        self.store_inode(inode)
    }

    pub fn replace_extents(
        &mut self,
        inode: &mut Inode,
        mut extents: Vec<Extent>,
    ) -> Result<(), Error> {
        let old_nodes = self.extent_blocks(&inode.root)?;
        for number in old_nodes {
            self.free_block(number)?;
        }
        extents.sort_unstable_by_key(|extent| extent.logical);
        let mut merged: Vec<Extent> = Vec::new();
        for extent in extents {
            if let Some(last) = merged.last_mut().filter(|last| {
                last.logical + last.length == extent.logical
                    && last.physical + last.length == extent.physical
                    && last.unwritten == extent.unwritten
                    && last.length + extent.length < 32768
            }) {
                last.length += extent.length;
            } else {
                merged.push(extent);
            }
        }
        let data_blocks: u64 = merged.iter().map(|extent| u64::from(extent.length)).sum();
        let mut leaves = Vec::new();
        let mut depth = 0;
        let mut tree_blocks = 0;
        if merged.len() > 4 {
            for chunk in merged.chunks(340) {
                let number = self.allocate_block()?;
                let mut bytes = Rc::new([0; BLOCK_SIZE]);
                extent_header(&mut Rc::make_mut(&mut bytes)[..], chunk.len(), 340, 0);
                for (index, extent) in chunk.iter().enumerate() {
                    extent_entry(&mut Rc::make_mut(&mut bytes)[..], 12 + index * 12, extent);
                }
                self.stage(number, bytes);
                leaves.push((chunk[0].logical, number));
                tree_blocks += 1;
            }
            depth = 1;
            while leaves.len() > 4 {
                let mut parents = Vec::new();
                for chunk in leaves.chunks(340) {
                    let number = self.allocate_block()?;
                    let mut bytes = Rc::new([0; BLOCK_SIZE]);
                    extent_header(&mut Rc::make_mut(&mut bytes)[..], chunk.len(), 340, depth);
                    for (index, &(logical, child)) in chunk.iter().enumerate() {
                        extent_index(
                            &mut Rc::make_mut(&mut bytes)[..],
                            12 + index * 12,
                            logical,
                            child,
                        );
                    }
                    self.stage(number, bytes);
                    parents.push((chunk[0].0, number));
                    tree_blocks += 1;
                }
                leaves = parents;
                depth += 1;
            }
        }
        inode.root.fill(0);
        if depth == 0 {
            extent_header(&mut inode.root, merged.len(), 4, 0);
            for (index, extent) in merged.iter().enumerate() {
                extent_entry(&mut inode.root, 12 + index * 12, extent);
            }
        } else {
            extent_header(&mut inode.root, leaves.len(), 4, depth);
            for (index, &(logical, child)) in leaves.iter().enumerate() {
                extent_index(&mut inode.root, 12 + index * 12, logical, child);
            }
        }
        inode.sectors = (data_blocks + tree_blocks) * 8;
        Ok(())
    }

    pub fn store_inode(&mut self, inode: &Inode) -> Result<(), Error> {
        self.edit_inode(inode.number, |bytes| {
            bytes[0..2].copy_from_slice(&inode.mode.to_le_bytes());
            bytes[2..4].copy_from_slice(&(inode.uid as u16).to_le_bytes());
            bytes[24..26].copy_from_slice(&(inode.gid as u16).to_le_bytes());
            bytes[120..122].copy_from_slice(&((inode.uid >> 16) as u16).to_le_bytes());
            bytes[122..124].copy_from_slice(&((inode.gid >> 16) as u16).to_le_bytes());
            store_timestamp(bytes, 8, 140, inode.accessed)?;
            store_timestamp(bytes, 16, 136, inode.modified)?;
            store_timestamp(bytes, 12, 132, inode.changed)?;
            bytes[4..8].copy_from_slice(&(inode.size as u32).to_le_bytes());
            bytes[26..28].copy_from_slice(&inode.links.to_le_bytes());
            bytes[28..32].copy_from_slice(&(inode.sectors as u32).to_le_bytes());
            bytes[32..36].copy_from_slice(&inode.flags.to_le_bytes());
            bytes[40..100].copy_from_slice(&inode.root);
            bytes[108..112].copy_from_slice(&((inode.size >> 32) as u32).to_le_bytes());
            Ok(())
        })
    }
}

fn store_timestamp(
    bytes: &mut [u8],
    offset: usize,
    extra: usize,
    time: crate::Timestamp,
) -> Result<(), Error> {
    let base = time.seconds as i32;
    let epoch = time
        .seconds
        .checked_sub(i64::from(base))
        .ok_or(Error::Unsupported)?
        >> 32;
    let extended =
        bytes.len() >= extra + 4 && usize::from(super::half(bytes, 128)) >= extra + 4 - 128;
    if time.nanoseconds >= 1_000_000_000
        || !(0..=3).contains(&epoch)
        || !extended && (epoch != 0 || time.nanoseconds != 0)
    {
        return Err(Error::Unsupported);
    }
    bytes[offset..offset + 4].copy_from_slice(&base.to_le_bytes());
    if extended {
        bytes[extra..extra + 4]
            .copy_from_slice(&((time.nanoseconds << 2) | epoch as u32).to_le_bytes());
    }
    Ok(())
}

fn extent_header(bytes: &mut [u8], count: usize, max: usize, depth: u16) {
    bytes[..2].copy_from_slice(&0xf30au16.to_le_bytes());
    bytes[2..4].copy_from_slice(&(count as u16).to_le_bytes());
    bytes[4..6].copy_from_slice(&(max as u16).to_le_bytes());
    bytes[6..8].copy_from_slice(&depth.to_le_bytes());
}
fn extent_entry(bytes: &mut [u8], offset: usize, extent: &Extent) {
    bytes[offset..offset + 4].copy_from_slice(&extent.logical.to_le_bytes());
    let length = extent.length as u16 + if extent.unwritten { 32768 } else { 0 };
    bytes[offset + 4..offset + 6].copy_from_slice(&length.to_le_bytes());
    bytes[offset + 8..offset + 12].copy_from_slice(&extent.physical.to_le_bytes());
}
fn extent_index(bytes: &mut [u8], offset: usize, logical: u32, child: u32) {
    bytes[offset..offset + 4].copy_from_slice(&logical.to_le_bytes());
    bytes[offset + 4..offset + 8].copy_from_slice(&child.to_le_bytes());
}
