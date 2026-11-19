// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

/// Size of a leaf-level-one mapping on the supported ISAs.
pub const HUGE_PAGE_SIZE: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u64)]
pub enum AddressMode {
    Bounds = 0,
    Containing = 1,
    AtOrAfter = 2,
}

pub struct AddressQuery {
    pub mode: AddressMode,
    pub address: u64,
}

impl AddressQuery {
    pub const SIZE: usize = 16;
    pub fn encode(self) -> [u8; Self::SIZE] {
        let mut bytes = [0; Self::SIZE];
        bytes[..8].copy_from_slice(&(self.mode as u64).to_le_bytes());
        bytes[8..].copy_from_slice(&self.address.to_le_bytes());
        bytes
    }
    pub fn decode(bytes: &[u8; Self::SIZE]) -> Result<Self, crate::Error> {
        let mode = match u64::from_le_bytes(bytes[..8].try_into().unwrap()) {
            0 => AddressMode::Bounds,
            1 => AddressMode::Containing,
            2 => AddressMode::AtOrAfter,
            _ => return Err(crate::Error::InvalidArgument),
        };
        let address = u64::from_le_bytes(bytes[8..].try_into().unwrap());
        if mode == AddressMode::Bounds && address != 0 {
            return Err(crate::Error::InvalidArgument);
        }
        Ok(Self { mode, address })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u64)]
pub enum RegionKind {
    Bounds = 0,
    Reserved = 1,
    Mapping = 2,
}

/// One address-space snapshot with an exclusive end address.
/// Reserved and bounds records have no permissions and a zero leaf level.
pub struct AddressRegion {
    pub start: u64,
    pub end: u64,
    pub kind: RegionKind,
    pub permissions: Option<Permissions>,
    pub leaf_level: u64,
}

impl AddressRegion {
    pub const SIZE: usize = 40;
    pub fn encode(self) -> [u8; Self::SIZE] {
        let mut bytes = [0; Self::SIZE];
        for (output, value) in bytes.as_chunks_mut::<8>().0.iter_mut().zip([
            self.start,
            self.end,
            self.kind as u64,
            self.permissions.map_or(0, |permissions| permissions as u64),
            self.leaf_level,
        ]) {
            *output = value.to_le_bytes();
        }
        bytes
    }
    pub fn decode(bytes: &[u8; Self::SIZE]) -> Result<Self, crate::Error> {
        let words = bytes.as_chunks::<8>().0;
        let [start, end, kind, permissions, leaf_level] =
            core::array::from_fn(|index| u64::from_le_bytes(words[index]));
        let kind = match kind {
            0 => RegionKind::Bounds,
            1 => RegionKind::Reserved,
            2 => RegionKind::Mapping,
            _ => return Err(crate::Error::InvalidArgument),
        };
        if start >= end || kind != RegionKind::Mapping && (permissions != 0 || leaf_level != 0) {
            return Err(crate::Error::InvalidArgument);
        }
        let permissions = if kind == RegionKind::Mapping {
            Some(Permissions::from_raw(permissions).ok_or(crate::Error::InvalidArgument)?)
        } else {
            None
        };
        Ok(Self {
            start,
            end,
            kind,
            permissions,
            leaf_level,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u64)]
pub enum Permissions {
    NoAccess = 8,
    ReadOnly = 1,
    ReadWrite = 3,
    ExecuteOnly = 4,
    ReadExecute = 5,
    ReadWriteExecute = 7,
}
impl Permissions {
    /// Rights required on the FrameAllocation supplied to Task::MAP.
    pub const fn mapping_rights(self) -> crate::Rights {
        use crate::Rights;
        match self {
            Self::NoAccess => Rights::MAP,
            Self::ReadOnly => Rights::MAP.union(Rights::READ),
            Self::ReadWrite => Rights::MAP.union(Rights::READ_WRITE),
            Self::ExecuteOnly => Rights::MAP.union(Rights::EXECUTE),
            Self::ReadExecute => Rights::MAP.union(Rights::READ).union(Rights::EXECUTE),
            Self::ReadWriteExecute => Rights::MAP.union(Rights::READ_WRITE).union(Rights::EXECUTE),
        }
    }

    pub const fn from_raw(raw: u64) -> Option<Self> {
        match raw {
            8 => Some(Self::NoAccess),
            1 => Some(Self::ReadOnly),
            3 => Some(Self::ReadWrite),
            4 => Some(Self::ExecuteOnly),
            5 => Some(Self::ReadExecute),
            7 => Some(Self::ReadWriteExecute),
            _ => None,
        }
    }
}

/// Mapping access and leaf level packed into Task::MAP's fifth parameter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MapMode {
    pub permissions: Permissions,
    pub leaf_level: u8,
}
impl MapMode {
    pub const fn encode(self) -> u64 {
        self.permissions as u64 | ((self.leaf_level as u64) << 8)
    }
    pub const fn decode(raw: u64) -> Option<Self> {
        if raw & !0xff0f != 0 {
            return None;
        }
        let Some(permissions) = Permissions::from_raw(raw & 0xf) else {
            return None;
        };
        Some(Self {
            permissions,
            leaf_level: (raw >> 8) as u8,
        })
    }
}
