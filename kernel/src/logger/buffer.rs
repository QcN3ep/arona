// SPDX-License-Identifier: MPL-2.0

//! Bounded log formatting and ring storage of whole records.

use core::{
    fmt::{self, Write},
    sync::atomic::{AtomicU64, Ordering},
};

use abi::logger::{HEADER_SIZE, MAX_RECORD_SIZE, Record};
use arrayvec::ArrayString;
use spin::Mutex;

const BUFFER_SIZE: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadError {
    Busy,
    BufferTooSmall,
}

pub(super) struct Buffer {
    sequence: AtomicU64,
    ring: Mutex<Ring>,
}

struct Ring {
    bytes: [u8; BUFFER_SIZE],
    head: usize,
    length: usize,
}

struct Text {
    bytes: ArrayString<{ MAX_RECORD_SIZE - HEADER_SIZE }>,
    truncated: bool,
}

impl Buffer {
    pub const fn new() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            ring: Mutex::new(Ring {
                bytes: [0; BUFFER_SIZE],
                head: 0,
                length: 0,
            }),
        }
    }

    pub fn write(&self, record: &log::Record<'_>, timestamp_ns: Option<u64>) {
        let mut text = Text {
            bytes: ArrayString::new_const(),
            truncated: false,
        };
        let _ = text.write_str(record.target());
        let target_length = text.bytes.len();
        if fmt::write(&mut text, *record.args()).is_err() {
            text.truncated = true;
        }
        let (target, message) = text.bytes.split_at(target_length);
        let mut bytes = [0; MAX_RECORD_SIZE];
        let ring = self.ring.try_lock();
        // Allocate the sequence number under the lock so published records remain increasing; records that are not written leave gaps in the sequence.
        let sequence = self
            .sequence
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1).filter(|next| *next < u64::MAX)
            });
        if let (Some(mut ring), Ok(sequence)) = (ring, sequence) {
            let length = Record {
                sequence: sequence + 1,
                timestamp_ns,
                level: match record.level() {
                    log::Level::Error => abi::logger::Level::Error,
                    log::Level::Warn => abi::logger::Level::Warn,
                    log::Level::Info => abi::logger::Level::Info,
                    log::Level::Debug => abi::logger::Level::Debug,
                    log::Level::Trace => abi::logger::Level::Trace,
                },
                truncated: text.truncated,
                target,
                message,
            }
            .encode(&mut bytes)
            .expect("log record exceeds its capacity");
            ring.push(&bytes[..length]);
        }
    }

    pub fn read(&self, cursor: u64, output: &mut [u8]) -> Result<usize, ReadError> {
        let ring = self.ring.try_lock().ok_or(ReadError::Busy)?;
        let mut offset = 0;
        while offset < ring.length {
            let length = ring.record_length(offset);
            let mut sequence = [0; 8];
            ring.copy(offset + 2, &mut sequence);
            if u64::from_le_bytes(sequence) >= cursor {
                let output = output.get_mut(..length).ok_or(ReadError::BufferTooSmall)?;
                ring.copy(offset + 2, output);
                return Ok(length);
            }
            offset += 2 + length;
        }
        Ok(0)
    }
}

impl Ring {
    fn record_length(&self, offset: usize) -> usize {
        let mut bytes = [0; 2];
        self.copy(offset, &mut bytes);
        u16::from_le_bytes(bytes) as usize
    }

    fn copy(&self, offset: usize, output: &mut [u8]) {
        let start = (self.head + offset) % BUFFER_SIZE;
        let first = output.len().min(BUFFER_SIZE - start);
        output[..first].copy_from_slice(&self.bytes[start..start + first]);
        let remaining = output.len() - first;
        output[first..].copy_from_slice(&self.bytes[..remaining]);
    }

    fn append(&mut self, bytes: &[u8]) {
        let start = (self.head + self.length) % BUFFER_SIZE;
        let first = bytes.len().min(BUFFER_SIZE - start);
        self.bytes[start..start + first].copy_from_slice(&bytes[..first]);
        self.bytes[..bytes.len() - first].copy_from_slice(&bytes[first..]);
        self.length += bytes.len();
    }

    fn push(&mut self, bytes: &[u8]) {
        while BUFFER_SIZE - self.length < bytes.len() + 2 {
            let discarded = self.record_length(0) + 2;
            self.head = (self.head + discarded) % BUFFER_SIZE;
            self.length -= discarded;
        }
        self.append(&(bytes.len() as u16).to_le_bytes());
        self.append(bytes);
    }
}

impl Write for Text {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let remaining = self.bytes.capacity() - self.bytes.len();
        let length = text.floor_char_boundary(remaining.min(text.len()));
        self.bytes.push_str(&text[..length]);
        if length < text.len() {
            self.truncated = true;
            Err(fmt::Error)
        } else {
            Ok(())
        }
    }
}
