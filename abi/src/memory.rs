// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

/// Large leaf size supported by the kernel page tables and the huge page filesystem.
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
