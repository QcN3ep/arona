// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Linux IP address and socket option value copying.

use crate::{
    process::Process,
    syscall::{abi::*, wait},
};

pub(super) fn input(
    process: &Process<'_>,
    address: usize,
    length: usize,
) -> Result<net::Endpoint, Errno> {
    if (length as i32) < 2 {
        return Err(EINVAL);
    }
    let family = wait::read_bytes(process, address, 2)?;
    // REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/include/uapi/linux/in6.h?h=v6.12
    let (size, ipv6) = match u16::from_le_bytes(family.try_into().unwrap()) {
        2 => (16, false),
        10 => (28, true),
        _ => return Err(EAFNOSUPPORT),
    };
    if length < size {
        return Err(EINVAL);
    }
    let bytes = wait::read_bytes(process, address, size)?;
    let address = if ipv6 {
        // TODO: Support scoped addresses when external IPv6 interfaces are attached.
        if bytes[24..28] != [0; 4] {
            return Err(EINVAL);
        }
        let address = net::Ipv6Addr::from(<[u8; 16]>::try_from(&bytes[8..24]).unwrap());
        if address.to_ipv4_mapped().is_some() {
            return Err(EAFNOSUPPORT);
        }
        net::IpAddr::V6(address)
    } else {
        net::IpAddr::V4(net::Ipv4Addr::from(
            <[u8; 4]>::try_from(&bytes[4..8]).unwrap(),
        ))
    };
    Ok(net::Endpoint {
        address,
        port: u16::from_be_bytes(bytes[2..4].try_into().unwrap()),
    })
}

pub(super) fn optional_input(
    process: &Process<'_>,
    address: usize,
    length: usize,
) -> Result<Option<net::Endpoint>, Errno> {
    (address != 0)
        .then(|| input(process, address, length))
        .transpose()
}

pub(in crate::syscall) struct Output {
    address: usize,
    length: usize,
    capacity: usize,
}

impl Output {
    pub fn with_capacity(address: usize, length: usize, capacity: usize) -> Self {
        Self {
            address,
            length,
            capacity,
        }
    }

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
        let mut bytes = [0; 28];
        bytes[2..4].copy_from_slice(&endpoint.port.to_be_bytes());
        let length = match endpoint.address {
            net::IpAddr::V4(address) => {
                bytes[..2].copy_from_slice(&2u16.to_le_bytes());
                bytes[4..8].copy_from_slice(&address.octets());
                16
            }
            net::IpAddr::V6(address) => {
                bytes[..2].copy_from_slice(&10u16.to_le_bytes());
                bytes[8..24].copy_from_slice(&address.octets());
                28
            }
        };
        self.copy(process, &bytes[..length])
    }

    pub fn value(&self, process: &Process<'_>, value: usize) -> SyscallResult {
        let bytes = (value as u32).to_le_bytes();
        self.copy(process, &bytes[..bytes.len().min(self.capacity)])
    }

    pub fn bytes(&self, process: &Process<'_>, bytes: &[u8]) -> SyscallResult {
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

pub(super) fn route(endpoint: net::Endpoint) -> Result<(), Errno> {
    // TODO: Route non-loopback destinations after external interfaces are attached.
    if endpoint.address.is_loopback() {
        Ok(())
    } else {
        Err(ENETUNREACH)
    }
}

pub(super) fn check_family(
    endpoint: net::Endpoint,
    socket: &crate::file::socket::Socket<'_>,
) -> Result<(), Errno> {
    if endpoint.family() == socket.family {
        Ok(())
    } else {
        Err(EAFNOSUPPORT)
    }
}
