// SPDX-License-Identifier: MPL-2.0

use super::{half, volume::Volume, word};
use crate::{Error, block::BLOCK_SIZE};
use alloc::{collections::BTreeSet, vec::Vec};

#[derive(Clone)]
pub(super) struct Inode {
    pub number: u32,
    pub size: u64,
    pub mode: u16,
    pub links: u16,
    pub sectors: u64,
    pub orphan_next: u32,
    pub root: [u8; 60],
}

#[derive(Clone)]
pub(super) struct Entry {
    pub inode: u32,
    pub name: Vec<u8>,
    pub file_type: u8,
}

impl Inode {
    pub fn parse(number: u32, bytes: &[u8]) -> Result<Self, Error> {
        let size = u64::from(word(bytes, 4)) | u64::from(word(bytes, 108)) << 32;
        let mode = half(bytes, 0);
        if !matches!(mode & 0xf000, 0x8000 | 0x4000) {
            return Err(Error::Unsupported);
        }
        if word(bytes, 32) != 0x80000
            || word(bytes, 20) != 0 && half(bytes, 26) != 0
            || word(bytes, 104) != 0
            || size > 2 * 1024 * 1024 * 1024
            || bytes.len() == 256 && half(bytes, 128) > 128
        {
            return Err(Error::InvalidData);
        }
        Ok(Self {
            number,
            size,
            mode,
            links: half(bytes, 26),
            sectors: u64::from(word(bytes, 28)),
            orphan_next: word(bytes, 20),
            root: bytes[40..100].try_into().unwrap(),
        })
    }

    pub fn directory(&self) -> bool {
        self.mode & 0xf000 == 0x4000
    }
}

impl Volume {
    pub fn read(
        &mut self,
        inode: &Inode,
        offset: usize,
        output: &mut [u8],
    ) -> Result<usize, Error> {
        let count = output
            .len()
            .min((inode.size as usize).saturating_sub(offset));
        let extents = self.extents(&inode.root)?;
        let mut done = 0;
        while done < count {
            let position = offset + done;
            let logical = (position / BLOCK_SIZE) as u32;
            let inside = position % BLOCK_SIZE;
            let length = (BLOCK_SIZE - inside).min(count - done);
            let extent = extents
                .iter()
                .find(|extent| (extent.logical..extent.logical + extent.length).contains(&logical));
            if let Some(extent) = extent.filter(|extent| !extent.unwritten) {
                let bytes = self.block(extent.physical + logical - extent.logical)?;
                output[done..done + length].copy_from_slice(&bytes[inside..inside + length]);
            } else {
                output[done..done + length].fill(0);
            }
            done += length;
        }
        Ok(count)
    }

    // REFERENCE: https://www.kernel.org/doc/html/v6.12/filesystems/ext4/directory.html
    pub fn entries(&mut self, inode: &Inode) -> Result<Vec<Entry>, Error> {
        if !inode.directory() || !inode.size.is_multiple_of(BLOCK_SIZE as u64) {
            return Err(Error::InvalidData);
        }
        let mut result = Vec::new();
        let mut names = BTreeSet::new();
        for offset in (0..inode.size as usize).step_by(BLOCK_SIZE) {
            let mut bytes = [0; BLOCK_SIZE];
            if self.read(inode, offset, &mut bytes)? != BLOCK_SIZE {
                return Err(Error::InvalidData);
            }
            let mut inside = 0;
            while inside < BLOCK_SIZE {
                if BLOCK_SIZE - inside < 8 {
                    return Err(Error::InvalidData);
                }
                let number = word(&bytes, inside);
                let length = usize::from(half(&bytes, inside + 4));
                let name_length = usize::from(bytes[inside + 6]);
                if length < 8
                    || !length.is_multiple_of(4)
                    || inside + length > BLOCK_SIZE
                    || name_length > length - 8
                    || number > self.inodes
                    || bytes[inside + 7] > 7
                {
                    return Err(Error::InvalidData);
                }
                if number != 0 {
                    let name = &bytes[inside + 8..inside + 8 + name_length];
                    if name.is_empty()
                        || name.contains(&0)
                        || name.contains(&b'/')
                        || !names.insert(name.to_vec())
                        || name == b"." && number != inode.number
                    {
                        return Err(Error::InvalidData);
                    }
                    result.push(Entry {
                        inode: number,
                        name: name.to_vec(),
                        file_type: bytes[inside + 7],
                    });
                }
                inside += length;
            }
        }
        if !names.contains(b".".as_slice()) || !names.contains(b"..".as_slice()) {
            return Err(Error::InvalidData);
        }
        Ok(result)
    }
}
