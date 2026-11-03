// SPDX-License-Identifier: MPL-2.0

use crate::Error;
use alloc::vec::Vec;

pub(crate) fn components(path: &[u8]) -> Result<Vec<&[u8]>, Error> {
    if path.is_empty() || path.len() > 4096 || path.contains(&0) {
        return Err(Error::InvalidRequest);
    }
    let mut result = Vec::new();
    for part in path.split(|byte| *byte == b'/') {
        if part.is_empty() || part == b"." {
            continue;
        }
        if part == b".." || part.len() > 255 {
            return Err(Error::InvalidRequest);
        }
        result.push(part);
    }
    Ok(result)
}

pub(crate) fn normalize(path: &[u8]) -> Result<Vec<u8>, Error> {
    let parts = components(path)?;
    Ok(parts.join(&b'/'))
}
