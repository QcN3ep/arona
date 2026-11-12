// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Linux IPv4 address and socket option value copying.

use crate::{
    process::Process,
    syscall::{abi::*, wait},
};

pub(super) fn input(
    process: &Process<'_>,
    address: usize,
    length: usize,
) -> Result<net::Endpoint, Errno> {
    if (length as i32) < 16 {
        return Err(EINVAL);
    }
    let bytes = wait::read_bytes(process, address, 16)?;
    if u16::from_le_bytes(bytes[..2].try_into().unwrap()) != 2 {
        return Err(EAFNOSUPPORT);
    }
    Ok(net::Endpoint {
        address: bytes[4..8].try_into().unwrap(),
        port: u16::from_be_bytes(bytes[2..4].try_into().unwrap()),
    })
}

pub(in crate::syscall) struct Output {
    address: usize,
    length: usize,
    capacity: usize,
}

impl Output {
    pub fn new(process: &Process<'_>, address: usize, length: usize) -> Result<Self, Errno> {
        let bytes = wait::read_bytes(process, length, 4)?;
        let capacity = i32::from_le_bytes(bytes.try_into().unwrap());
        if capacity < 0 {
            return Err(EINVAL);
        }
        Ok(Self {
            address,
            length,
            capacity: capacity as usize,
        })
    }

    pub fn endpoint(&self, process: &Process<'_>, endpoint: net::Endpoint) -> SyscallResult {
        let mut bytes = [0; 16];
        bytes[..2].copy_from_slice(&2u16.to_le_bytes());
        bytes[2..4].copy_from_slice(&endpoint.port.to_be_bytes());
        bytes[4..8].copy_from_slice(&endpoint.address);
        self.copy(process, &bytes)
    }

    pub fn value(&self, process: &Process<'_>, value: usize) -> SyscallResult {
        let bytes = (value as u32).to_le_bytes();
        self.copy(process, &bytes[..bytes.len().min(self.capacity)])
    }

    fn copy(&self, process: &Process<'_>, bytes: &[u8]) -> SyscallResult {
        wait::write_bytes(
            process,
            self.address,
            &bytes[..bytes.len().min(self.capacity)],
        )?;
        wait::write_bytes(process, self.length, &(bytes.len() as u32).to_le_bytes())?;
        Ok(0)
    }
}

pub(super) fn option(level: usize, name: usize) -> Result<net::OptionName, Errno> {
    match (level as u32, name as u32) {
        (1, 2) => Ok(net::OptionName::ReuseAddress),
        (6, 1) => Ok(net::OptionName::NoDelay),
        // TODO: Support socket buffers, keepalive, and I/O timeouts.
        _ => Err(ENOPROTOOPT),
    }
}
