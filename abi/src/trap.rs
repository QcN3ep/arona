// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserState {
    pub pc: usize,
    pub registers: [usize; 32],
    pub floating: [u64; 32],
    pub floating_control: u32,
    pub floating_conditions: [u8; 8],
}

impl UserState {
    pub const SIZE: usize = 532;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut bytes = [0; Self::SIZE];
        for (chunk, word) in bytes[..520].as_chunks_mut::<8>().0.iter_mut().zip(
            core::iter::once(self.pc as u64)
                .chain(self.registers.iter().map(|word| *word as u64))
                .chain(self.floating.iter().copied()),
        ) {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        bytes[520..524].copy_from_slice(&self.floating_control.to_le_bytes());
        bytes[524..].copy_from_slice(&self.floating_conditions);
        bytes
    }

    pub fn decode(bytes: &[u8; Self::SIZE]) -> Self {
        let word =
            |index: usize| u64::from_le_bytes(bytes[index * 8..index * 8 + 8].try_into().unwrap());
        Self {
            pc: word(0) as usize,
            registers: core::array::from_fn(|index| word(index + 1) as usize),
            floating: core::array::from_fn(|index| word(index + 33)),
            floating_control: u32::from_le_bytes(bytes[520..524].try_into().unwrap()),
            floating_conditions: bytes[524..].try_into().unwrap(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrapInfo {
    pub thread: u64,
    pub kind: u64,
    pub pc: u64,
    pub successor: u64,
    pub detail: u64,
    pub address: u64,
}

impl TrapInfo {
    pub const SYSCALL: u64 = 0;
    pub const PAGE_FAULT: u64 = 1;
    pub const ILLEGAL_INSTRUCTION: u64 = 2;
    pub const EXCEPTION: u64 = 3;
    pub const INTERRUPTED: u64 = 4;
    pub const SIZE: usize = 48;

    pub fn encode(self) -> [u8; Self::SIZE] {
        let mut bytes = [0; Self::SIZE];
        for (chunk, word) in bytes.as_chunks_mut::<8>().0.iter_mut().zip([
            self.thread,
            self.kind,
            self.pc,
            self.successor,
            self.detail,
            self.address,
        ]) {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        bytes
    }

    pub fn decode(bytes: &[u8; Self::SIZE]) -> Option<Self> {
        let word =
            |index: usize| u64::from_le_bytes(bytes[index * 8..index * 8 + 8].try_into().unwrap());
        let info = Self {
            thread: word(0),
            kind: word(1),
            pc: word(2),
            successor: word(3),
            detail: word(4),
            address: word(5),
        };
        (info.kind <= Self::INTERRUPTED).then_some(info)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u64)]
pub enum FaultAccess {
    Read = 0,
    Write = 1,
    Execute = 2,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u64)]
pub enum ExceptionClass {
    MisalignedAddress = 0,
    AccessFault = 1,
    BoundsCheck = 2,
    Breakpoint = 3,
    Arithmetic = 4,
    Privilege = 5,
    Unknown = 6,
}

impl ExceptionClass {
    /// 从陷入信息中的原始类别值还原异常类别。
    pub const fn from_raw(value: u64) -> Option<Self> {
        Some(match value {
            0 => Self::MisalignedAddress,
            1 => Self::AccessFault,
            2 => Self::BoundsCheck,
            3 => Self::Breakpoint,
            4 => Self::Arithmetic,
            5 => Self::Privilege,
            6 => Self::Unknown,
            _ => return None,
        })
    }
}
