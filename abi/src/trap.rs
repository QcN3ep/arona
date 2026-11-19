// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

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
    /// Decodes one defined exception class.
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
