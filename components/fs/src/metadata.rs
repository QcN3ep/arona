// SPDX-License-Identifier: MPL-2.0

use crate::Error;
use core::sync::atomic::{AtomicU64, Ordering};

static NEXT_DEVICE: AtomicU64 = AtomicU64::new(1);

pub(crate) fn device_identity() -> Result<u64, Error> {
    let mut id = NEXT_DEVICE.load(Ordering::Relaxed);
    loop {
        let next = id.checked_add(1).ok_or(Error::NoSpace)?;
        match NEXT_DEVICE.compare_exchange_weak(id, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Ok(id),
            Err(current) => id = current,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Timestamp {
    pub seconds: i64,
    pub nanoseconds: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Metadata {
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    pub blocks: u64,
    pub links: u32,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub accessed: Timestamp,
    pub modified: Timestamp,
    pub changed: Timestamp,
}

impl Metadata {
    pub const SIZE: usize = 84;

    pub fn encode(self) -> [u8; Self::SIZE] {
        let mut bytes = [0; Self::SIZE];
        for (chunk, word) in
            bytes[..24]
                .as_chunks_mut::<8>()
                .0
                .iter_mut()
                .zip([self.inode, self.size, self.blocks])
        {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        for (chunk, word) in bytes[24..40]
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip([self.links, self.mode, self.uid, self.gid])
        {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        for (chunk, time) in bytes[40..76].as_chunks_mut::<12>().0.iter_mut().zip([
            self.accessed,
            self.modified,
            self.changed,
        ]) {
            chunk[..8].copy_from_slice(&time.seconds.to_le_bytes());
            chunk[8..].copy_from_slice(&time.nanoseconds.to_le_bytes());
        }
        bytes[76..84].copy_from_slice(&self.device.to_le_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8; Self::SIZE]) -> Result<Self, Error> {
        let word =
            |offset: usize| u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
        let half =
            |offset: usize| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        let time = |offset: usize| Timestamp {
            seconds: word(offset) as i64,
            nanoseconds: half(offset + 8),
        };
        let info = Self {
            device: word(76),
            inode: word(0),
            size: word(8),
            blocks: word(16),
            links: half(24),
            mode: half(28),
            uid: half(32),
            gid: half(36),
            accessed: time(40),
            modified: time(52),
            changed: time(64),
        };
        info.validate()?;
        Ok(info)
    }

    pub(crate) fn validate(&self) -> Result<(), Error> {
        if !matches!(self.mode & 0o170000, 0o040000 | 0o100000 | 0o120000)
            || self.mode & !0o177777 != 0
            || [self.accessed, self.modified, self.changed]
                .iter()
                .any(|time| time.nanoseconds >= 1_000_000_000)
        {
            return Err(Error::InvalidRequest);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Attributes {
    pub permissions: u32,
    pub uid: u32,
    pub gid: u32,
    pub accessed: Timestamp,
    pub modified: Timestamp,
    pub changed: Timestamp,
}

impl Attributes {
    pub const SIZE: usize = 48;

    pub fn encode(self) -> [u8; Self::SIZE] {
        let mut bytes = [0; Self::SIZE];
        for (chunk, word) in bytes[..12].as_chunks_mut::<4>().0.iter_mut().zip([
            self.permissions,
            self.uid,
            self.gid,
        ]) {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        for (chunk, time) in bytes[12..].as_chunks_mut::<12>().0.iter_mut().zip([
            self.accessed,
            self.modified,
            self.changed,
        ]) {
            chunk[..8].copy_from_slice(&time.seconds.to_le_bytes());
            chunk[8..].copy_from_slice(&time.nanoseconds.to_le_bytes());
        }
        bytes
    }

    pub fn decode(bytes: &[u8; Self::SIZE]) -> Result<Self, Error> {
        let word =
            |offset: usize| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        let time = |offset: usize| Timestamp {
            seconds: i64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap()),
            nanoseconds: word(offset + 8),
        };
        let attributes = Self {
            permissions: word(0),
            uid: word(4),
            gid: word(8),
            accessed: time(12),
            modified: time(24),
            changed: time(36),
        };
        attributes.validate()?;
        Ok(attributes)
    }

    pub(crate) fn validate(&self) -> Result<(), Error> {
        if self.permissions & !0o7777 != 0
            || [self.accessed, self.modified, self.changed]
                .iter()
                .any(|time| time.nanoseconds >= 1_000_000_000)
        {
            return Err(Error::InvalidRequest);
        }
        Ok(())
    }
}

impl Metadata {
    pub fn attributes(self) -> Attributes {
        Attributes {
            permissions: self.mode & 0o7777,
            uid: self.uid,
            gid: self.gid,
            accessed: self.accessed,
            modified: self.modified,
            changed: self.changed,
        }
    }
}
