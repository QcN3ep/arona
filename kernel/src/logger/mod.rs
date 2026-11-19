// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Log facade, kernel log buffer, and output Sinks.

mod buffer;

pub(crate) use buffer::CAPACITY;
pub use buffer::ReadError;

use core::fmt;
use core::sync::atomic::{AtomicUsize, Ordering};

use log::{LevelFilter, Log, Metadata, Record, SetLoggerError};
use spin::{Once, RwLock};

use buffer::Buffer;

/// Both output and flush must be bounded and must not wait for other writers when reentrant or in interrupt context.
pub trait Sink: Sync {
    fn write(&self, record: &Record<'_>) -> fmt::Result;

    /// On success, pending output must have been drained; on failure, retrying must be possible.
    fn flush(&self) -> fmt::Result {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SinkError {
    Busy,
    Full,
    Flush,
}

impl fmt::Display for SinkError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str(match self {
            Self::Busy => "sink is busy",
            Self::Full => "sink capacity exhausted",
            Self::Flush => "failed to flush sink",
        })
    }
}

pub struct Logger {
    max_level: AtomicUsize,
    sink: RwLock<Option<&'static dyn Sink>>,
    buffer: Buffer,
    clock: Once<fn() -> Option<u64>>,
}

#[derive(Clone)]
pub struct KernelLog {
    logger: &'static Logger,
}

impl KernelLog {
    /// With a zero cursor, reads start from the earliest record; returns zero when there are no records and does not modify any read position.
    pub fn read(&self, cursor: u64, output: &mut [u8]) -> Result<usize, ReadError> {
        self.logger.buffer.read(cursor, output)
    }
}

impl Logger {
    pub const fn new(max_level: LevelFilter) -> Self {
        Self {
            max_level: AtomicUsize::new(max_level as usize),
            sink: RwLock::new(None),
            buffer: Buffer::new(),
            clock: Once::new(),
        }
    }

    pub fn set_sink(&self, sink: &'static dyn Sink) -> Result<(), SinkError> {
        let mut current = self.sink.try_write().ok_or(SinkError::Busy)?;
        if current.is_some() {
            return Err(SinkError::Full);
        }
        *current = Some(sink);
        Ok(())
    }

    pub fn remove_sink(&self) -> Result<(), SinkError> {
        let mut current = self.sink.try_write().ok_or(SinkError::Busy)?;
        if let Some(sink) = *current {
            sink.flush().map_err(|_| SinkError::Flush)?;
            *current = None;
        }
        Ok(())
    }

    pub fn set_max_level(&self, max_level: LevelFilter) {
        self.max_level.store(max_level as usize, Ordering::Relaxed);
    }

    pub fn kernel_log(&'static self) -> KernelLog {
        KernelLog { logger: self }
    }

    pub fn enable_timestamps(&self, clock: fn() -> Option<u64>) {
        self.clock.call_once(|| clock);
    }

    fn dispatch(&self, mut output: impl FnMut(&dyn Sink) -> fmt::Result) {
        if let Some(current) = self.sink.try_read()
            && let Some(sink) = *current
        {
            let _ = output(sink);
        }
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() as usize <= self.max_level.load(Ordering::Relaxed)
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            self.buffer
                .write(record, self.clock.get().and_then(|clock| clock()));
            self.dispatch(|sink| sink.write(record));
        }
    }

    fn flush(&self) {
        self.dispatch(|sink| sink.flush());
    }
}

pub static LOGGER: Logger = Logger::new(LevelFilter::Off);

pub fn init(max_level: LevelFilter) -> Result<(), SetLoggerError> {
    log::set_logger(&LOGGER)?;
    LOGGER.set_max_level(max_level);
    log::set_max_level(LevelFilter::Trace);
    Ok(())
}
