// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! File open, create, and truncate options.

use crate::Error;

#[derive(Clone, Copy, Debug)]
pub struct OpenOptions {
    pub path_only: bool,
    pub write: bool,
    pub create: bool,
    pub exclusive: bool,
    pub truncate: bool,
    pub permissions: u16,
}

impl OpenOptions {
    pub(crate) fn validate(self) -> Result<(), Error> {
        if self.path_only && (self.write || self.create || self.exclusive || self.truncate) {
            return Err(Error::InvalidRequest);
        }
        if self.permissions & !0o7777 != 0 || self.exclusive && !self.create {
            return Err(Error::InvalidRequest);
        }
        Ok(())
    }

    pub(crate) fn encode(self) -> [u64; 2] {
        [
            u64::from(self.write)
                | u64::from(self.create) << 1
                | u64::from(self.exclusive) << 2
                | u64::from(self.truncate) << 3
                | u64::from(self.path_only) << 4,
            u64::from(self.permissions),
        ]
    }

    pub(crate) fn decode(flags: u64, permissions: u64) -> Result<Self, Error> {
        if flags & !31 != 0 || permissions > u64::from(u16::MAX) {
            return Err(Error::InvalidRequest);
        }
        let options = Self {
            path_only: flags & 16 != 0,
            write: flags & 1 != 0,
            create: flags & 2 != 0,
            exclusive: flags & 4 != 0,
            truncate: flags & 8 != 0,
            permissions: permissions as u16,
        };
        options.validate()?;
        Ok(options)
    }
}
