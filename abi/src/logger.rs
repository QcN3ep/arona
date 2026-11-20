// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Encoding of kernel log records.

pub const MAX_RECORD_SIZE: usize = 1024;
pub const HEADER_SIZE: usize = 32;

const TRUNCATED: u32 = 1;
const TIMESTAMP: u32 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Level {
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
    Trace = 5,
}

impl Level {
    pub fn from_raw(value: u32) -> Option<Self> {
        match value {
            1 => Some(Self::Error),
            2 => Some(Self::Warn),
            3 => Some(Self::Info),
            4 => Some(Self::Debug),
            5 => Some(Self::Trace),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Record<'a> {
    /// The sequence starts at one; a gap indicates a lost record.
    pub sequence: u64,
    /// Nanoseconds since clock initialization, or None when the clock is unavailable.
    pub timestamp_ns: Option<u64>,
    pub level: Level,
    pub truncated: bool,
    pub target: &'a str,
    pub message: &'a str,
}

impl Record<'_> {
    pub fn encode(self, output: &mut [u8]) -> Option<usize> {
        let length = HEADER_SIZE
            .checked_add(self.target.len())?
            .checked_add(self.message.len())?;
        if self.sequence == 0 || self.sequence == u64::MAX || length > MAX_RECORD_SIZE {
            return None;
        }
        let output = output.get_mut(..length)?;
        let flags = (u32::from(self.truncated) * TRUNCATED)
            | (u32::from(self.timestamp_ns.is_some()) * TIMESTAMP);
        output[..8].copy_from_slice(&self.sequence.to_le_bytes());
        output[8..16].copy_from_slice(&self.timestamp_ns.unwrap_or(0).to_le_bytes());
        for (bytes, word) in output[16..HEADER_SIZE]
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip([
                self.level as u32,
                flags,
                self.target.len() as u32,
                self.message.len() as u32,
            ])
        {
            bytes.copy_from_slice(&word.to_le_bytes());
        }
        let (target, message) = output[HEADER_SIZE..].split_at_mut(self.target.len());
        target.copy_from_slice(self.target.as_bytes());
        message.copy_from_slice(self.message.as_bytes());
        Some(length)
    }
}

impl<'a> Record<'a> {
    pub fn decode(bytes: &'a [u8]) -> Option<Self> {
        if bytes.len() > MAX_RECORD_SIZE {
            return None;
        }
        let (record, length) = Self::decode_prefix(bytes)?;
        (length == bytes.len()).then_some(record)
    }

    /// Decodes the first complete record and returns the number of bytes it occupies.
    pub fn decode_prefix(bytes: &'a [u8]) -> Option<(Self, usize)> {
        let header = bytes.get(..HEADER_SIZE)?;
        let sequence = u64::from_le_bytes(header[..8].try_into().ok()?);
        let timestamp = u64::from_le_bytes(header[8..16].try_into().ok()?);
        let mut words = [0; 4];
        for (word, bytes) in words.iter_mut().zip(header[16..].as_chunks::<4>().0) {
            *word = u32::from_le_bytes(*bytes);
        }
        let level = Level::from_raw(words[0])?;
        let flags = words[1];
        if sequence == 0 || sequence == u64::MAX || flags & !(TRUNCATED | TIMESTAMP) != 0 {
            return None;
        }
        let length = HEADER_SIZE
            .checked_add(words[2] as usize)?
            .checked_add(words[3] as usize)?;
        if length > MAX_RECORD_SIZE {
            return None;
        }
        let payload = bytes.get(HEADER_SIZE..length)?;
        let (target, message) = payload.split_at(words[2] as usize);
        Some((
            Self {
                sequence,
                timestamp_ns: (flags & TIMESTAMP != 0).then_some(timestamp),
                level,
                truncated: flags & TRUNCATED != 0,
                target: core::str::from_utf8(target).ok()?,
                message: core::str::from_utf8(message).ok()?,
            },
            length,
        ))
    }
}
