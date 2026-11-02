// SPDX-License-Identifier: MPL-2.0

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u64)]
pub enum DmaProfile {
    Isolated = 0,
    CompetitionTrusted = 1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DmaInfo {
    pub profile: DmaProfile,
    pub address_bits: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u64)]
pub enum DmaDirection {
    ToDevice = 1,
    FromDevice = 2,
    Bidirectional = 3,
}

impl DmaDirection {
    pub const fn from_raw(raw: u64) -> Option<Self> {
        match raw {
            1 => Some(Self::ToDevice),
            2 => Some(Self::FromDevice),
            3 => Some(Self::Bidirectional),
            _ => None,
        }
    }

    pub const fn required_rights(self) -> crate::Rights {
        crate::Rights::DMA_MAP.union(match self {
            Self::ToDevice => crate::Rights::READ,
            Self::FromDevice => crate::Rights::WRITE,
            Self::Bidirectional => crate::Rights::READ_WRITE,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u64)]
pub enum DmaState {
    Quiesced = 0,
    Active = 1,
    Revoked = 2,
    Poisoned = 3,
}

impl DmaState {
    pub const fn from_raw(raw: u64) -> Option<Self> {
        match raw {
            0 => Some(Self::Quiesced),
            1 => Some(Self::Active),
            2 => Some(Self::Revoked),
            3 => Some(Self::Poisoned),
            _ => None,
        }
    }
}
