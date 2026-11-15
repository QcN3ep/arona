// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! ext4 metadata checksums using the filesystem and inode seeds.

// REFERENCE: https://www.kernel.org/doc/html/v6.12/filesystems/ext4/checksums.html

use super::{half, journal::crc32c, word};
use crate::Error;

#[derive(Clone, Copy)]
pub(super) struct Checksum {
    pub seed: u32,
    pub enabled: bool,
}

impl Checksum {
    pub fn superblock(self, bytes: &mut [u8]) {
        if self.enabled {
            let sum = crc32c(!0, &bytes[..1020]);
            bytes[1020..1024].copy_from_slice(&sum.to_le_bytes());
        }
    }

    pub fn group(self, index: u32, bytes: &[u8]) -> u16 {
        let crc = crc32c(self.seed, &index.to_le_bytes());
        let crc = crc32c(crc, &bytes[..30]);
        let crc = crc32c(crc, &[0; 2]);
        crc32c(crc, &bytes[32..]) as u16
    }

    pub fn inode_seed(self, number: u32, generation: u32) -> u32 {
        crc32c(
            crc32c(self.seed, &number.to_le_bytes()),
            &generation.to_le_bytes(),
        )
    }

    fn inode_sum(self, number: u32, bytes: &[u8]) -> u32 {
        let crc = self.inode_seed(number, word(bytes, 100));
        let crc = crc32c(crc, &bytes[..124]);
        let crc = crc32c(crc, &[0; 2]);
        if bytes.len() > 128 && half(bytes, 128) >= 4 {
            let crc = crc32c(crc, &bytes[126..130]);
            crc32c(crc32c(crc, &[0; 2]), &bytes[132..])
        } else {
            crc32c(crc, &bytes[126..])
        }
    }

    pub fn verify_inode(self, number: u32, bytes: &[u8]) -> Result<(), Error> {
        if !self.enabled {
            return Ok(());
        }
        let mut sum = self.inode_sum(number, bytes);
        let mut stored = u32::from(half(bytes, 124));
        if bytes.len() > 128 && half(bytes, 128) >= 4 {
            stored |= u32::from(half(bytes, 130)) << 16;
        } else {
            sum &= 0xffff;
        }
        if sum != stored {
            return Err(Error::InvalidData);
        }
        Ok(())
    }

    pub fn seal_inode(self, number: u32, bytes: &mut [u8]) {
        if self.enabled {
            let sum = self.inode_sum(number, bytes);
            bytes[124..126].copy_from_slice(&(sum as u16).to_le_bytes());
            if bytes.len() > 128 && half(bytes, 128) >= 4 {
                bytes[130..132].copy_from_slice(&((sum >> 16) as u16).to_le_bytes());
            }
        }
    }
    pub fn directory(
        self,
        number: u32,
        generation: u32,
        bytes: &[u8],
        index: Option<usize>,
    ) -> Result<usize, Error> {
        if !self.enabled {
            return Ok(bytes.len());
        }
        let seed = self.inode_seed(number, generation);
        if let Some(offset) = index {
            let limit = usize::from(half(bytes, offset));
            let count = usize::from(half(bytes, offset + 2));
            let tail = offset + limit * 8;
            if count == 0 || count > limit || tail + 8 != bytes.len() || word(bytes, tail) != 0 {
                return Err(Error::InvalidData);
            }
            let crc = crc32c(seed, &bytes[..offset + count * 8]);
            let crc = crc32c(crc, &bytes[tail..tail + 4]);
            if crc32c(crc, &[0; 4]) != word(bytes, tail + 4) {
                return Err(Error::InvalidData);
            }
            Ok(bytes.len())
        } else {
            let tail = bytes.len() - 12;
            if word(bytes, tail) != 0
                || half(bytes, tail + 4) != 12
                || bytes[tail + 6] != 0
                || bytes[tail + 7] != 0xde
                || crc32c(seed, &bytes[..tail]) != word(bytes, tail + 8)
            {
                return Err(Error::InvalidData);
            }
            Ok(tail)
        }
    }

    pub fn seal_directory(self, number: u32, generation: u32, bytes: &mut [u8]) {
        if self.enabled {
            let tail = bytes.len() - 12;
            bytes[tail..].fill(0);
            bytes[tail + 4..tail + 6].copy_from_slice(&12u16.to_le_bytes());
            bytes[tail + 7] = 0xde;
            let sum = crc32c(self.inode_seed(number, generation), &bytes[..tail]);
            bytes[tail + 8..].copy_from_slice(&sum.to_le_bytes());
        }
    }

    pub fn extent(self, seed: u32, bytes: &[u8]) -> Result<(), Error> {
        if self.enabled {
            let tail = 12 + usize::from(half(bytes, 4)) * 12;
            if tail + 4 > bytes.len() || crc32c(seed, &bytes[..tail]) != word(bytes, tail) {
                return Err(Error::InvalidData);
            }
        }
        Ok(())
    }

    pub fn seal_extent(self, seed: u32, bytes: &mut [u8]) {
        if self.enabled {
            let tail = 12 + usize::from(half(bytes, 4)) * 12;
            let sum = crc32c(seed, &bytes[..tail]);
            bytes[tail..tail + 4].copy_from_slice(&sum.to_le_bytes());
        }
    }
}
