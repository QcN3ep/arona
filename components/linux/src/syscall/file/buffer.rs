// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Userspace I/O buffer snapshots and scattered copying.

use alloc::vec::Vec;

use super::super::abi::{EFAULT, EINVAL, Errno, SyscallResult};
use crate::{elf::PAGE_SIZE, process::Process};

// REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/include/uapi/linux/uio.h?h=v6.12
const IOV_MAX: usize = 1024;

pub(in crate::syscall) enum Buffer {
    Slice { address: usize, length: usize },
    Vector { address: usize, count: usize },
}

pub(in crate::syscall) struct IoVectors {
    vectors: Vec<IoVector>,
}

struct IoVector {
    address: usize,
    length: usize,
}

impl Buffer {
    pub(super) fn import(self, process: &Process<'_>) -> Result<IoVectors, Errno> {
        let vectors = match self {
            Self::Slice { address, length } => {
                if length > isize::MAX as usize {
                    return Err(EINVAL);
                }
                alloc::vec![IoVector { address, length }]
            }
            Self::Vector { address, count } => {
                if count > IOV_MAX {
                    return Err(EINVAL);
                }
                let mut bytes = alloc::vec![0; count * 16];
                if count != 0 {
                    process
                        .address_space
                        .read(address, &mut bytes)
                        .map_err(|_| EFAULT)?;
                }
                let mut total = 0usize;
                let mut vectors = Vec::with_capacity(count);
                for entry in bytes.as_chunks::<16>().0 {
                    let address = u64::from_le_bytes(entry[..8].try_into().unwrap()) as usize;
                    let length = u64::from_le_bytes(entry[8..].try_into().unwrap()) as usize;
                    total = total
                        .checked_add(length)
                        .filter(|total| *total <= isize::MAX as usize)
                        .ok_or(EINVAL)?;
                    vectors.push(IoVector { address, length });
                }
                vectors
            }
        };
        Ok(IoVectors { vectors })
    }
}

impl IoVectors {
    pub fn total_length(&self) -> usize {
        self.vectors.iter().map(|vector| vector.length).sum()
    }

    pub fn length(&self) -> usize {
        self.vectors
            .iter()
            .map(|vector| vector.length)
            .sum::<usize>()
            .min(PAGE_SIZE)
    }

    pub fn gather(&self, process: &Process<'_>, limit: usize) -> Result<Vec<u8>, Errno> {
        let mut bytes = alloc::vec![0; self.length().min(limit)];
        let count = self.copy(&mut bytes, |address, output| {
            process.address_space.read(address, output)
        })?;
        bytes.truncate(count);
        Ok(bytes)
    }

    pub fn scatter(&self, process: &Process<'_>, bytes: &mut [u8]) -> SyscallResult {
        self.copy(bytes, |address, input| {
            process.address_space.write(address, input)
        })
    }

    fn copy(
        &self,
        bytes: &mut [u8],
        mut copy: impl FnMut(usize, &mut [u8]) -> Result<(), runtime::Error>,
    ) -> SyscallResult {
        let mut copied = 0;
        for vector in &self.vectors {
            let mut inside = 0;
            while inside < vector.length && copied < bytes.len() {
                let result = vector
                    .address
                    .checked_add(inside)
                    .ok_or(EFAULT)
                    .and_then(|address| {
                        let length = (vector.length - inside)
                            .min(bytes.len() - copied)
                            .min(PAGE_SIZE - address % PAGE_SIZE);
                        copy(address, &mut bytes[copied..copied + length]).map_err(|_| EFAULT)?;
                        Ok(length)
                    });
                match result {
                    Ok(length) => {
                        copied += length;
                        inside += length;
                    }
                    Err(error) if copied == 0 => return Err(error),
                    Err(_) => return Ok(copied),
                }
            }
        }
        Ok(copied)
    }
}
