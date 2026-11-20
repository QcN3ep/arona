// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Mounted volume identities, roots, and capacity information.

use crate::{Error, Metadata, Statistics};
use alloc::vec::Vec;

pub struct Volume {
    pub path: Vec<u8>,
    pub root: Metadata,
    pub statistics: Statistics,
}

impl Volume {
    pub(crate) const MAX_BYTES: usize = 8192;
    const HEADER: usize = Metadata::SIZE + Statistics::SIZE;

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut bytes = self.root.encode().to_vec();
        bytes.extend_from_slice(&self.statistics.encode());
        bytes.extend_from_slice(&self.path);
        bytes
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() <= Self::HEADER || bytes.len() >= Self::MAX_BYTES {
            return Err(Error::InvalidData);
        }
        let path = &bytes[Self::HEADER..];
        if !path.starts_with(b"/") || path.len() >= 4096 || path.contains(&0) {
            return Err(Error::InvalidData);
        }
        let root = Metadata::decode(bytes[..Metadata::SIZE].try_into().unwrap())?;
        if crate::FileType::from_mode(root.mode) != crate::FileType::Directory {
            return Err(Error::InvalidData);
        }
        Ok(Self {
            path: path.to_vec(),
            root,
            statistics: Statistics::decode(
                bytes[Metadata::SIZE..Self::HEADER].try_into().unwrap(),
            )?,
        })
    }
}
