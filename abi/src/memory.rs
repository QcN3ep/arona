// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

/// Size of a leaf-level-one mapping on the supported ISAs.
pub const HUGE_PAGE_SIZE: usize = 2 * 1024 * 1024;

/// Mapping records the kernel admits for user mappings in total.
pub const MAX_USER_MAPPING_RECORDS: usize = 65536;

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

    pub const fn writable(self) -> bool {
        matches!(self, Self::ReadWrite | Self::ReadWriteExecute)
    }

    pub const fn executable(self) -> bool {
        matches!(
            self,
            Self::ExecuteOnly | Self::ReadExecute | Self::ReadWriteExecute
        )
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
