// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! ISA register groups and little-endian context records.

pub const GENERAL: u64 = 1;
pub const FLOAT: u64 = 2;

/// Borrowed group contents for an atomic update; this view is not a wire layout.
#[derive(Clone, Copy)]
pub struct Write<'a> {
    pub group: u64,
    pub bytes: &'a [u8],
}

#[derive(Clone, Copy)]
pub struct WriteRequest {
    pub group: u64,
    pub address: u64,
    pub length: u64,
}
impl WriteRequest {
    pub const SIZE: usize = 24;
    pub fn encode(self) -> [u8; Self::SIZE] {
        let mut bytes = [0; Self::SIZE];
        for (output, value) in
            bytes
                .as_chunks_mut::<8>()
                .0
                .iter_mut()
                .zip([self.group, self.address, self.length])
        {
            *output = value.to_le_bytes();
        }
        bytes
    }
    pub fn decode(bytes: &[u8; Self::SIZE]) -> Self {
        let words = bytes.as_chunks::<8>().0;
        Self {
            group: u64::from_le_bytes(words[0]),
            address: u64::from_le_bytes(words[1]),
            length: u64::from_le_bytes(words[2]),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct General {
    pub pc: u64,
    pub registers: [u64; 32],
}
impl General {
    pub const SIZE: usize = 264;
    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut bytes = [0; Self::SIZE];
        for (output, value) in bytes
            .as_chunks_mut::<8>()
            .0
            .iter_mut()
            .zip(core::iter::once(self.pc).chain(self.registers))
        {
            *output = value.to_le_bytes();
        }
        bytes
    }
    pub fn decode(bytes: &[u8; Self::SIZE]) -> Self {
        let words = bytes.as_chunks::<8>().0;
        Self {
            pc: u64::from_le_bytes(words[0]),
            registers: core::array::from_fn(|index| u64::from_le_bytes(words[index + 1])),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScalarFloat {
    pub registers: [u64; 32],
    pub control: u32,
    pub conditions: [u8; 8],
}
impl ScalarFloat {
    pub const RISCV_SIZE: usize = 264;
    pub const LOONGARCH_SIZE: usize = 272;
    pub fn encode(
        &self,
        length: usize,
    ) -> Result<([u8; Self::LOONGARCH_SIZE], usize), crate::Error> {
        if !matches!(length, Self::RISCV_SIZE | Self::LOONGARCH_SIZE)
            || length == Self::RISCV_SIZE && self.conditions != [0; 8]
        {
            return Err(crate::Error::InvalidArgument);
        }
        let mut bytes = [0; Self::LOONGARCH_SIZE];
        for (output, value) in bytes[..256]
            .as_chunks_mut::<8>()
            .0
            .iter_mut()
            .zip(self.registers)
        {
            *output = value.to_le_bytes();
        }
        bytes[256..260].copy_from_slice(&self.control.to_le_bytes());
        bytes[264..].copy_from_slice(&self.conditions);
        Ok((bytes, length))
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, crate::Error> {
        if !matches!(bytes.len(), Self::RISCV_SIZE | Self::LOONGARCH_SIZE)
            || bytes[260..264] != [0; 4]
        {
            return Err(crate::Error::InvalidArgument);
        }
        let words = bytes[..256].as_chunks::<8>().0;
        let mut conditions = [0; 8];
        if bytes.len() == Self::LOONGARCH_SIZE {
            conditions.copy_from_slice(&bytes[264..]);
        }
        Ok(Self {
            registers: core::array::from_fn(|index| u64::from_le_bytes(words[index])),
            control: u32::from_le_bytes(bytes[256..260].try_into().unwrap()),
            conditions,
        })
    }
}
