// SPDX-License-Identifier: MPL-2.0

//! 内核日志记录的编码。

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Record<'a> {
    /// 序号从一开始，空缺表示记录丢失。
    pub sequence: u64,
    /// 时钟初始化后的纳秒数，时钟不可用时为 None。
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
        if !(HEADER_SIZE..=MAX_RECORD_SIZE).contains(&bytes.len()) {
            return None;
        }
        let sequence = u64::from_le_bytes(bytes[..8].try_into().ok()?);
        let timestamp = u64::from_le_bytes(bytes[8..16].try_into().ok()?);
        let mut words = [0; 4];
        for (word, bytes) in words
            .iter_mut()
            .zip(bytes[16..HEADER_SIZE].as_chunks::<4>().0)
        {
            *word = u32::from_le_bytes(*bytes);
        }
        let level = match words[0] {
            1 => Level::Error,
            2 => Level::Warn,
            3 => Level::Info,
            4 => Level::Debug,
            5 => Level::Trace,
            _ => return None,
        };
        let flags = words[1];
        if sequence == 0 || sequence == u64::MAX || flags & !(TRUNCATED | TIMESTAMP) != 0 {
            return None;
        }
        let payload = &bytes[HEADER_SIZE..];
        if (words[2] as usize).checked_add(words[3] as usize)? != payload.len() {
            return None;
        }
        let (target, message) = payload.split_at(words[2] as usize);
        Some(Self {
            sequence,
            timestamp_ns: (flags & TIMESTAMP != 0).then_some(timestamp),
            level,
            truncated: flags & TRUNCATED != 0,
            target: core::str::from_utf8(target).ok()?,
            message: core::str::from_utf8(message).ok()?,
        })
    }
}
