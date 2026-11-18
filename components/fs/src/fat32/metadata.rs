// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::{
    Node, half,
    volume::{ROOT, Volume},
};
use crate::{Attributes, Error, Metadata, Timestamp};
use alloc::rc::Rc;
use core::cell::RefCell;

impl Node {
    pub(super) fn metadata(&self, volume: &Volume) -> Metadata {
        let directory = self.entry.directory();
        let size = self.entry.size() as u64;
        let raw = &self.entry.raw;
        let modified = decode(half(raw, 24), half(raw, 22), 0);
        Metadata {
            device: volume.device,
            mount: volume.mount,
            inode: self.inode,
            size,
            blocks: size.div_ceil(volume.cluster_size() as u64)
                * (volume.cluster_size() as u64 / 512),
            links: u32::from(self.linked),
            mode: if directory {
                if self.entry.read_only() {
                    0o040555
                } else {
                    0o040755
                }
            } else if self.entry.read_only() {
                0o100444
            } else {
                0o100644
            },
            accessed: decode(half(raw, 18), 0, 0),
            modified,
            changed: modified,
            birth: decode(half(raw, 16), half(raw, 14), raw[13]),
            ..Metadata::default()
        }
    }
}

impl Volume {
    pub fn set_attributes(
        &mut self,
        node: &Rc<RefCell<Node>>,
        attributes: Attributes,
    ) -> Result<(), Error> {
        self.check_writable()?;
        attributes.validate()?;
        let mut entry = node.borrow().entry.clone();
        let current = node.borrow().metadata(self);
        let writable = if entry.directory() { 0o755 } else { 0o644 };
        if attributes.uid != 0
            || attributes.gid != 0
            || ![writable, writable & !0o222].contains(&attributes.permissions)
            || (attributes.changed != current.changed && attributes.changed != attributes.modified)
            || entry.location == ROOT
        {
            return Err(Error::Unsupported);
        }
        if attributes.modified != current.modified {
            let (date, time) = encode(attributes.modified)?;
            entry.raw[22..24].copy_from_slice(&time.to_le_bytes());
            entry.raw[24..26].copy_from_slice(&date.to_le_bytes());
        }
        if attributes.accessed != current.accessed {
            let (access, _) = encode(attributes.accessed)?;
            entry.raw[18..20].copy_from_slice(&access.to_le_bytes());
        }
        entry.raw[11] = (entry.raw[11] & !1) | u8::from(attributes.permissions & 0o222 == 0);
        if node.borrow().linked {
            self.transaction(|volume| volume.store_entry(&entry))?;
        }
        node.borrow_mut().entry = entry;
        Ok(())
    }
}

fn leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn month_days(year: i64, month: usize) -> i64 {
    [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ][month - 1]
}

fn decode(date: u16, time: u16, fraction: u8) -> Timestamp {
    let year = 1980 + i64::from(date >> 9);
    let month = usize::from((date >> 5) & 15);
    let day = i64::from(date & 31);
    if !(1..=12).contains(&month)
        || day < 1
        || day > month_days(year, month)
        || time >> 11 > 23
        || (time >> 5) & 63 > 59
        || time & 31 > 29
        || fraction > 199
    {
        return Timestamp::default();
    }
    let days = (1970..year)
        .map(|year| if leap(year) { 366 } else { 365 })
        .sum::<i64>()
        + (1..month).map(|month| month_days(year, month)).sum::<i64>()
        + day
        - 1;
    Timestamp {
        seconds: days * 86400
            + i64::from(time >> 11) * 3600
            + i64::from((time >> 5) & 63) * 60
            + i64::from(time & 31) * 2
            + i64::from(fraction / 100),
        nanoseconds: u32::from(fraction % 100) * 10_000_000,
    }
}

fn encode(stamp: Timestamp) -> Result<(u16, u16), Error> {
    if stamp.seconds < 315532800 || stamp.seconds >= 4354819200 {
        return Err(Error::InvalidRequest);
    }
    let mut days = stamp.seconds / 86400;
    let mut year = 1970;
    loop {
        let count = if leap(year) { 366 } else { 365 };
        if days < count {
            break;
        }
        days -= count;
        year += 1;
    }
    let mut month = 1;
    while days >= month_days(year, month) {
        days -= month_days(year, month);
        month += 1;
    }
    let seconds = stamp.seconds % 86400;
    Ok((
        (((year - 1980) << 9) | ((month as i64) << 5) | (days + 1)) as u16,
        (((seconds / 3600) << 11) | (((seconds / 60) % 60) << 5) | ((seconds % 60) / 2)) as u16,
    ))
}
