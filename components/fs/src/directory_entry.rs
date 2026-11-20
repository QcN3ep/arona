// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Directory entry records exchanged by the file service.

use crate::{DirEntry, Error, FileType};

impl DirEntry {
    pub(crate) const HEADER_SIZE: usize = 11;

    /// Returns no record when the remaining output cannot hold this entry.
    pub(crate) fn encode_into(&self, output: &mut [u8]) -> Result<Option<usize>, Error> {
        if self.name.is_empty() || self.name.len() > 255 {
            return Err(Error::InvalidData);
        }
        let length = Self::HEADER_SIZE + self.name.len();
        if output.len() < length {
            return Ok(None);
        }
        output[..8].copy_from_slice(&self.inode.to_le_bytes());
        output[8] = match self.kind {
            FileType::Unknown => 0,
            FileType::Regular => 1,
            FileType::Directory => 2,
            FileType::SymbolicLink => 3,
        };
        output[9..Self::HEADER_SIZE].copy_from_slice(&(self.name.len() as u16).to_le_bytes());
        output[Self::HEADER_SIZE..length].copy_from_slice(&self.name);
        Ok(Some(length))
    }

    pub(crate) fn decode_prefix(bytes: &[u8]) -> Result<(Self, usize), Error> {
        if bytes.len() < Self::HEADER_SIZE {
            return Err(Error::InvalidData);
        }
        let kind = match bytes[8] {
            0 => FileType::Unknown,
            1 => FileType::Regular,
            2 => FileType::Directory,
            3 => FileType::SymbolicLink,
            _ => return Err(Error::InvalidData),
        };
        let size = usize::from(u16::from_le_bytes([bytes[9], bytes[10]]));
        let length = Self::HEADER_SIZE + size;
        if size == 0 || size > 255 || length > bytes.len() {
            return Err(Error::InvalidData);
        }
        Ok((
            Self {
                name: bytes[Self::HEADER_SIZE..length].to_vec(),
                inode: u64::from_le_bytes(bytes[..8].try_into().unwrap()),
                kind,
            },
            length,
        ))
    }
}
