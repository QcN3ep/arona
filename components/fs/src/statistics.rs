// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Filesystem capacity, identity, and name limits.

use crate::Error;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Statistics {
    pub kind: u64,
    pub device: u64,
    pub block_size: u64,
    pub blocks: u64,
    pub free_blocks: u64,
    pub available_blocks: u64,
    pub files: u64,
    pub free_files: u64,
    pub name_length: u64,
    pub flags: u64,
}

impl Statistics {
    pub(crate) const SIZE: usize = 80;

    pub(crate) fn encode(self) -> [u8; Self::SIZE] {
        let mut bytes = [0; Self::SIZE];
        for (output, word) in bytes.chunks_exact_mut(8).zip([
            self.kind,
            self.device,
            self.block_size,
            self.blocks,
            self.free_blocks,
            self.available_blocks,
            self.files,
            self.free_files,
            self.name_length,
            self.flags,
        ]) {
            output.copy_from_slice(&word.to_le_bytes());
        }
        bytes
    }

    pub(crate) fn decode(bytes: &[u8; Self::SIZE]) -> Result<Self, Error> {
        let words: [u64; 10] = core::array::from_fn(|index| {
            u64::from_le_bytes(bytes[index * 8..index * 8 + 8].try_into().unwrap())
        });
        let value = Self {
            kind: words[0],
            device: words[1],
            block_size: words[2],
            blocks: words[3],
            free_blocks: words[4],
            available_blocks: words[5],
            files: words[6],
            free_files: words[7],
            name_length: words[8],
            flags: words[9],
        };
        if value.block_size == 0
            || value.available_blocks > value.free_blocks
            || value.free_blocks > value.blocks
            || value.free_files > value.files
        {
            return Err(Error::InvalidData);
        }
        Ok(value)
    }
}
