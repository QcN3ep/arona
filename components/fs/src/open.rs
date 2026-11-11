// SPDX-License-Identifier: MPL-2.0

//! File open, create, and truncate options.

use crate::Error;

#[derive(Clone, Copy, Debug)]
pub struct OpenOptions {
    pub write: bool,
    pub create: bool,
    pub exclusive: bool,
    pub truncate: bool,
    pub permissions: u16,
}

impl OpenOptions {
    pub(crate) fn validate(self) -> Result<(), Error> {
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
                | u64::from(self.truncate) << 3,
            u64::from(self.permissions),
        ]
    }

    pub(crate) fn decode(flags: u64, permissions: u64) -> Result<Self, Error> {
        if flags & !15 != 0 || permissions > u64::from(u16::MAX) {
            return Err(Error::InvalidRequest);
        }
        let options = Self {
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
