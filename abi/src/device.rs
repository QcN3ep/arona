// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceInfo {
    pub id: u32,
    pub parent: Option<u32>,
    pub group: Option<u32>,
    pub mmio_regions: usize,
    pub interrupts: usize,
    pub identities: usize,
    pub properties: usize,
    pub console: bool,
    pub dma: crate::dma::DmaInfo,
}

impl DeviceInfo {
    pub const SIZE: usize = 80;

    pub fn encode(self) -> [u8; Self::SIZE] {
        let words = [
            self.id as u64,
            self.parent.unwrap_or(0) as u64,
            self.group.unwrap_or(0) as u64,
            self.mmio_regions as u64,
            self.interrupts as u64,
            self.identities as u64,
            self.properties as u64,
            u64::from(self.console),
            self.dma.profile as u64,
            u64::from(self.dma.address_bits),
        ];
        let mut bytes = [0; Self::SIZE];
        for (output, word) in bytes.as_chunks_mut::<8>().0.iter_mut().zip(words) {
            output.copy_from_slice(&word.to_le_bytes());
        }
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::SIZE {
            return None;
        }
        let mut words = [0u64; 10];
        for (word, input) in words.iter_mut().zip(bytes.as_chunks::<8>().0.iter()) {
            *word = u64::from_le_bytes(*input);
        }
        let parent = u32::try_from(words[1]).ok()?;
        let group = u32::try_from(words[2]).ok()?;
        Some(Self {
            id: u32::try_from(words[0]).ok().filter(|id| *id != 0)?,
            parent: (parent != 0).then_some(parent),
            group: (group != 0).then_some(group),
            mmio_regions: usize::try_from(words[3]).ok()?,
            interrupts: usize::try_from(words[4]).ok()?,
            identities: usize::try_from(words[5]).ok()?,
            properties: usize::try_from(words[6]).ok()?,
            console: match words[7] {
                0 => false,
                1 => true,
                _ => return None,
            },
            dma: crate::dma::DmaInfo {
                profile: match words[8] {
                    0 => crate::dma::DmaProfile::Isolated,
                    1 => crate::dma::DmaProfile::CompetitionTrusted,
                    _ => return None,
                },
                address_bits: u8::try_from(words[9]).ok().filter(|bits| *bits <= 64)?,
            },
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u64)]
pub enum DeviceField {
    Name = 0,
    IdentityNamespace = 1,
    Identity = 2,
    PropertyNamespace = 3,
    PropertyName = 4,
    PropertyValue = 5,
    PciConfig = 6,
    PciBar = 7,
}

impl DeviceField {
    pub const fn from_raw(raw: u64) -> Option<Self> {
        match raw {
            0 => Some(Self::Name),
            1 => Some(Self::IdentityNamespace),
            2 => Some(Self::Identity),
            3 => Some(Self::PropertyNamespace),
            4 => Some(Self::PropertyName),
            5 => Some(Self::PropertyValue),
            6 => Some(Self::PciConfig),
            7 => Some(Self::PciBar),
            _ => None,
        }
    }
}

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

    pub fn encode(self) -> [u8; Self::SIZE] {
        let mut bytes = [0; Self::SIZE];
        bytes[..4].copy_from_slice(&[
            self.index,
            self.width,
            u8::from(self.prefetchable),
            self.region.unwrap_or(u8::MAX),
        ]);
        bytes[8..16].copy_from_slice(&self.bus_address.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.size.to_le_bytes());
        bytes
    }

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
