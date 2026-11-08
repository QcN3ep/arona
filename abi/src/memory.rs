// SPDX-License-Identifier: MPL-2.0

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u64)]
pub enum Permissions {
    NoAccess = 8,
    ReadOnly = 1,
    ReadWrite = 3,
    ExecuteOnly = 4,
    ReadExecute = 5,
}
impl Permissions {
    pub const fn from_raw(raw: u64) -> Option<Self> {
        match raw {
            8 => Some(Self::NoAccess),
            1 => Some(Self::ReadOnly),
            3 => Some(Self::ReadWrite),
            4 => Some(Self::ExecuteOnly),
            5 => Some(Self::ReadExecute),
            _ => None,
        }
    }
}
