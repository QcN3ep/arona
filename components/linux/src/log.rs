// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Kernel log reader state shared by the Linux syslog call.

use alloc::{format, vec::Vec};
use core::cell::Cell;
use runtime::{
    Handle, KernelLog,
    logger::{MAX_RECORD_SIZE, Record},
};

use crate::syscall::abi::{EIO, ENODEV, EOVERFLOW, Errno};

/// Largest text snapshot retained for a single read. The kernel ring holds
/// 64 KiB of encoded records, so formatted output stays well below this limit.
const MAX_TEXT: usize = 1024 * 1024;

/// The reader position and the oldest record still visible to `syslog`.
///
/// `SYSLOG_ACTION_CLEAR` advances `floor` and `consumed`; it does not mutate the
/// kernel ring that other readers may still inspect.
pub struct Log {
    kernel: Option<Handle<KernelLog>>,
    floor: Cell<u64>,
    consumed: Cell<usize>,
}

impl Default for Log {
    fn default() -> Self {
        Self {
            kernel: None,
            floor: Cell::new(0),
            consumed: Cell::new(0),
        }
    }
}

impl Log {
    pub fn new(kernel: Option<Handle<KernelLog>>) -> Self {
        Self {
            kernel,
            floor: Cell::new(0),
            consumed: Cell::new(0),
        }
    }

    pub(crate) fn kernel(&self) -> Option<&Handle<KernelLog>> {
        self.kernel.as_ref()
    }

    pub(crate) fn take_kernel(&mut self) -> Option<Handle<KernelLog>> {
        self.kernel.take()
    }

    /// Read unread bytes without committing the cursor. The returned index is
    /// the new cursor to commit after the destination accepted the bytes.
    pub(crate) fn read(&self, length: usize) -> Result<(Vec<u8>, usize), Errno> {
        let (text, _) = self.scan()?;
        let start = self.consumed.get().min(text.len());
        let end = start.saturating_add(length).min(text.len());
        Ok((text[start..end].to_vec(), end))
    }

    pub(crate) fn commit_read(&self, end: usize) {
        self.consumed.set(end);
    }

    /// Read the last `length` bytes of the visible log without moving the reader.
    pub(crate) fn read_all(&self, length: usize) -> Result<Vec<u8>, Errno> {
        let (text, _) = self.scan()?;
        let start = text.len().saturating_sub(length);
        Ok(text[start..].to_vec())
    }

    pub(crate) fn size_unread(&self) -> Result<usize, Errno> {
        let (text, _) = self.scan()?;
        Ok(text.len().saturating_sub(self.consumed.get()))
    }

    /// Read the last `length` bytes without clearing. The returned cursor
    /// hides every record seen by this scan once committed.
    pub(crate) fn read_clear(&self, length: usize) -> Result<(Vec<u8>, u64), Errno> {
        let (text, next) = self.scan()?;
        let start = text.len().saturating_sub(length);
        Ok((text[start..].to_vec(), next))
    }

    pub(crate) fn commit_clear(&self, next: u64) {
        self.floor.set(next);
        self.consumed.set(0);
    }

    pub(crate) fn clear(&self) -> Result<(), Errno> {
        let (_, next) = self.scan()?;
        self.floor.set(next);
        self.consumed.set(0);
        Ok(())
    }

    /// Format every visible record and report the cursor following the newest one.
    fn scan(&self) -> Result<(Vec<u8>, u64), Errno> {
        let kernel = self.kernel.as_ref().ok_or(ENODEV)?;
        let mut cursor = self.floor.get();
        let mut text = Vec::new();
        let mut bytes = [0; MAX_RECORD_SIZE];
        while text.len() < MAX_TEXT {
            let record = kernel.read(cursor, &mut bytes).map_err(|_| EIO)?;
            let Some(record) = record else {
                break;
            };
            cursor = record.sequence.checked_add(1).ok_or(EOVERFLOW)?;
            append(&mut text, record);
        }
        Ok((text, cursor))
    }
}

fn append(text: &mut Vec<u8>, record: Record<'_>) {
    let timestamp = record.timestamp_ns.unwrap_or(0);
    let line = format!(
        "<{}>[{:5}.{:06}] {}: {}\n",
        record.level as u32,
        timestamp / 1_000_000_000,
        timestamp % 1_000_000_000 / 1_000,
        record.target,
        record.message,
    );
    text.extend_from_slice(line.as_bytes());
}
