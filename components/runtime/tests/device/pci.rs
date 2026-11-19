// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use runtime::{DeviceSet, Error};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PciBar {
    pub index: u8,
    pub width: u8,
    pub prefetchable: bool,
    pub region: Option<u8>,
    pub bus_address: u64,
    pub size: u64,
}

impl PciBar {
    pub const SIZE: usize = 24;

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let bytes: &[u8; Self::SIZE] = bytes.try_into().ok()?;
        let bar = Self {
            index: bytes[0],
            width: bytes[1],
            prefetchable: match bytes[2] {
                0 => false,
                1 => true,
                _ => return None,
            },
            region: (bytes[3] != u8::MAX).then_some(bytes[3]),
            bus_address: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
            size: u64::from_le_bytes(bytes[16..24].try_into().ok()?),
        };
        (bar.index < 6
            && matches!(bar.width, 32 | 64)
            && (bar.width != 64 || bar.index < 5)
            && bytes[4..8] == [0; 4]
            && bar.size.is_power_of_two()
            && bar.size >= 16
            && bar.bus_address.is_multiple_of(bar.size)
            && bar.bus_address.checked_add(bar.size).is_some()
            && (bar.width == 64 || bar.bus_address + bar.size <= 1 << 32))
            .then_some(bar)
    }
}

pub fn configuration(
    devices: &runtime::Handle<DeviceSet>,
    id: u32,
    output: &mut [u8; 4096],
) -> Result<(), Error> {
    if devices.configuration(id, output)? != output.len() {
        return Err(Error::Protocol);
    }
    Ok(())
}
pub fn pci_bar(
    devices: &runtime::Handle<DeviceSet>,
    id: u32,
    index: usize,
) -> Result<PciBar, Error> {
    if index >= 6 {
        return Err(Error::Protocol);
    }
    let name = [b'b', b'a', b'r', b'0' + index as u8];
    let mut bytes = [0; PciBar::SIZE];
    let bytes = devices
        .property(id, "pci", core::str::from_utf8(&name).unwrap(), &mut bytes)?
        .ok_or(Error::Kernel(abi::Error::Unsupported))?;
    PciBar::decode(bytes).ok_or(Error::Protocol)
}
