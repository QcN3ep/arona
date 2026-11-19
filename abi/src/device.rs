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
    pub dma_address_bits: u8,
}

impl DeviceInfo {
    pub const SIZE: usize = 64;

    pub fn encode(self) -> [u8; Self::SIZE] {
        let words = [
            self.id as u64,
            self.parent.unwrap_or(0) as u64,
            self.group.unwrap_or(0) as u64,
            self.mmio_regions as u64,
            self.interrupts as u64,
            self.identities as u64,
            self.properties as u64,
            u64::from(self.dma_address_bits),
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
        let mut words = [0u64; 8];
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
            dma_address_bits: u8::try_from(words[7]).ok().filter(|bits| *bits <= 64)?,
        })
    }
}
