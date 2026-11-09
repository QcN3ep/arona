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

// REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/fs/namei.c?h=v6.12
impl crate::namespace::Namespace {
    pub(crate) fn resolve(
        &self,
        base: &[u8],
        path: &[u8],
        follow: bool,
        missing: bool,
    ) -> Result<Vec<u8>, Error> {
        use alloc::collections::VecDeque;
        if path.is_empty() || path.contains(&0) {
            return Err(Error::InvalidRequest);
        }
        if path.len() >= 4096 {
            return Err(Error::NameTooLong);
        }
        let mut resolved: Vec<Vec<u8>> = if path.starts_with(b"/") {
            Vec::new()
        } else {
            components(base)?.into_iter().map(<[u8]>::to_vec).collect()
        };
        let mut pending: VecDeque<Vec<u8>> = path
            .split(|byte| *byte == b'/')
            .filter(|part| !part.is_empty())
            .map(<[u8]>::to_vec)
            .collect();
        let mut directory = path.ends_with(b"/");
        let mut links = 0;
        while let Some(part) = pending.pop_front() {
            if part.len() > 255 {
                return Err(Error::NameTooLong);
            }
            if part == b"." {
                continue;
            }
            if part == b".." {
                resolved.pop();
                continue;
            }
            resolved.push(part);
            let name = absolute(&resolved);
            if name.len() >= 4096 {
                return Err(Error::NameTooLong);
            }
            let info = match self.metadata(&name) {
                Err(Error::NotFound) if missing && pending.is_empty() && !directory => {
                    return Ok(name);
                }
                result => result?,
            };
            if info.mode & 0o170000 == 0o120000 && (follow || !pending.is_empty() || directory) {
                links += 1;
                if links > 40 {
                    return Err(Error::TooManyLinks);
                }
                let target = self.read_link(&name)?;
                if target.is_empty() {
                    return Err(Error::NotFound);
                }
                resolved.pop();
                if target.starts_with(b"/") {
                    resolved.clear();
                }
                if pending.is_empty() && target.ends_with(b"/") {
                    directory = true;
                }
                for part in target
                    .split(|byte| *byte == b'/')
                    .filter(|part| !part.is_empty())
                    .rev()
                {
                    pending.push_front(part.to_vec());
                }
            } else if (!pending.is_empty() || directory) && info.mode & 0o170000 != 0o040000 {
                return Err(Error::NotDirectory);
            }
        }
        Ok(absolute(&resolved))
    }
}

fn absolute(parts: &[Vec<u8>]) -> Vec<u8> {
    let mut path = alloc::vec![b'/'];
    path.extend(parts.join(&b'/'));
    path
}
