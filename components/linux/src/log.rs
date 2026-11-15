// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Kernel log reader state shared by the Linux syslog call.

use alloc::{format, vec::Vec};
use core::cell::Cell;
use runtime::{
    Handle, KernelLog,
    logger::{MAX_RECORD_SIZE, Record},
};

use crate::syscall::abi::{EIO, ENODEV, EOVERFLOW, Errno};

// The kernel ring holds 64 KiB of encoded records.
const MAX_TEXT: usize = 1024 * 1024;

#[derive(Clone, Copy, Default)]
pub(crate) struct Cursor {
    sequence: u64,
    offset: usize,
}

/// The consuming reader and the independent floor for READ_ALL and READ_CLEAR.
#[derive(Default)]
pub struct Log {
    kernel: Option<Handle<KernelLog>>,
    floor: Cell<u64>,
    consumed: Cell<Cursor>,
}

impl Log {
    pub fn new(kernel: Option<Handle<KernelLog>>) -> Self {
        Self {
            kernel,
            ..Self::default()
        }
    }

    pub(crate) fn kernel(&self) -> Option<&Handle<KernelLog>> {
        self.kernel.as_ref()
    }

    pub(crate) fn take_kernel(&mut self) -> Option<Handle<KernelLog>> {
        self.kernel.take()
    }

    /// Read unread bytes, returning a cursor to commit after the user copy succeeds.
    pub(crate) fn read(&self, length: usize) -> Result<(Vec<u8>, Cursor), Errno> {
        self.scan(self.consumed.get(), length)
    }

    pub(crate) fn commit_read(&self, end: Cursor) {
        self.consumed.set(end);
    }

    /// Read the last `length` visible bytes without moving the consuming reader.
    pub(crate) fn read_all(&self, length: usize) -> Result<Vec<u8>, Errno> {
        self.read_clear(length).map(|(bytes, _)| bytes)
    }

    pub(crate) fn size_unread(&self) -> Result<usize, Errno> {
        self.scan(self.consumed.get(), MAX_TEXT)
            .map(|(bytes, _)| bytes.len())
    }

    /// Return the visible tail and a clear floor to commit after the user copy succeeds.
    pub(crate) fn read_clear(&self, length: usize) -> Result<(Vec<u8>, u64), Errno> {
        let start = Cursor {
            sequence: self.floor.get(),
            offset: 0,
        };
        let (text, next) = self.scan(start, MAX_TEXT)?;
        let start = text.len().saturating_sub(length);
        Ok((text[start..].to_vec(), next.sequence))
    }

    pub(crate) fn commit_clear(&self, next: u64) {
        self.floor.set(next);
    }

    pub(crate) fn clear(&self) -> Result<(), Errno> {
        let (_, next) = self.read_clear(0)?;
        self.commit_clear(next);
        Ok(())
    }

    fn scan(&self, mut cursor: Cursor, length: usize) -> Result<(Vec<u8>, Cursor), Errno> {
        let kernel = self.kernel.as_ref().ok_or(ENODEV)?;
        let length = length.min(MAX_TEXT);
        let mut text = Vec::new();
        let mut bytes = [0; MAX_RECORD_SIZE];
        while text.len() < length {
            let Some(record) = kernel.read(cursor.sequence, &mut bytes).map_err(|_| EIO)? else {
                break;
            };
            if record.sequence != cursor.sequence {
                // An evicted record's byte offset cannot apply to its successor.
                cursor.offset = 0;
            }
            let next = record.sequence.checked_add(1).ok_or(EOVERFLOW)?;
            let line = format_record(record);
            let start = cursor.offset.min(line.len());
            let end = start + (line.len() - start).min(length - text.len());
            text.extend_from_slice(&line[start..end]);
            cursor = if end == line.len() {
                Cursor {
                    sequence: next,
                    offset: 0,
                }
            } else {
                Cursor {
                    sequence: record.sequence,
                    offset: end,
                }
            };
        }
        Ok((text, cursor))
    }
}

fn format_record(record: Record<'_>) -> alloc::string::String {
    let timestamp = record.timestamp_ns.unwrap_or(0);
    format!(
        "<{}>[{:5}.{:06}] {}: {}\n",
        record.level as u32,
        timestamp / 1_000_000_000,
        timestamp % 1_000_000_000 / 1_000,
        record.target,
        record.message,
    )
}
