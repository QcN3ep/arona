// SPDX-License-Identifier: MPL-2.0

//! 日志门面的实现

use core::fmt;
use core::sync::atomic::{AtomicUsize, Ordering};

use log::{LevelFilter, Log, Metadata, Record, SetLoggerError};
use spin::{RwLock, RwLockWriteGuard};

pub const MAX_SINKS: usize = 8;

/// 输出与 flush 均须有界，重入或中断上下文中不可等待其他写入者。
pub trait Sink: Sync {
    fn write(&self, record: &Record<'_>) -> fmt::Result;

    /// 成功时须已排空待输出数据，失败时须允许重试。
    fn flush(&self) -> fmt::Result {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SinkError {
    Busy,
    Full,
    Detached,
    Flush,
}

impl fmt::Display for SinkError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str(match self {
            Self::Busy => "sink is busy",
            Self::Full => "sink capacity exhausted",
            Self::Detached => "sink is detached",
            Self::Flush => "failed to flush sink",
        })
    }
}

struct Slot {
    generation: usize,
    sink: Option<&'static dyn Sink>,
}

/// 句柄失效后不得影响复用该槽位的新注册，丢弃句柄不会移除 Sink。
pub struct SinkHandle<'logger> {
    slot: &'logger RwLock<Slot>,
    generation: usize,
}

impl SinkHandle<'_> {
    pub fn replace(&self, sink: &'static dyn Sink) -> Result<&'static dyn Sink, SinkError> {
        let mut slot = self.lock()?;
        let old = slot.sink.ok_or(SinkError::Detached)?;
        old.flush().map_err(|_| SinkError::Flush)?;
        slot.sink = Some(sink);
        Ok(old)
    }

    pub fn remove(&self) -> Result<&'static dyn Sink, SinkError> {
        let mut slot = self.lock()?;
        let old = slot.sink.ok_or(SinkError::Detached)?;
        old.flush().map_err(|_| SinkError::Flush)?;
        slot.sink = None;
        Ok(old)
    }

    pub fn discard(&self) -> Result<&'static dyn Sink, SinkError> {
        self.lock()?.sink.take().ok_or(SinkError::Detached)
    }

    fn lock(&self) -> Result<RwLockWriteGuard<'_, Slot>, SinkError> {
        let slot = self.slot.try_write().ok_or(SinkError::Busy)?;
        if slot.generation != self.generation {
            return Err(SinkError::Detached);
        }
        Ok(slot)
    }
}

pub struct Logger {
    max_level: AtomicUsize,
    slots: [RwLock<Slot>; MAX_SINKS],
    failures: AtomicUsize,
}

impl Logger {
    pub const fn new(max_level: LevelFilter) -> Self {
        Self {
            max_level: AtomicUsize::new(max_level as usize),
            slots: [const {
                RwLock::new(Slot {
                    generation: 0,
                    sink: None,
                })
            }; MAX_SINKS],
            failures: AtomicUsize::new(0),
        }
    }

    pub fn add_sink(&self, sink: &'static dyn Sink) -> Result<SinkHandle<'_>, SinkError> {
        let mut busy = false;
        for slot in &self.slots {
            let Some(entry) = slot.try_read() else {
                busy = true;
                continue;
            };
            if entry.sink.is_some() {
                continue;
            }
            drop(entry);
            let Some(mut entry) = slot.try_write() else {
                busy = true;
                continue;
            };
            if entry.sink.is_some() {
                continue;
            }
            let Some(generation) = entry.generation.checked_add(1) else {
                continue;
            };
            entry.generation = generation;
            entry.sink = Some(sink);
            return Ok(SinkHandle { slot, generation });
        }
        Err(if busy {
            SinkError::Busy
        } else {
            SinkError::Full
        })
    }

    pub fn set_max_level(&self, max_level: LevelFilter) {
        self.max_level.store(max_level as usize, Ordering::Relaxed);
    }

    /// 写入与 flush 分发失败的次数，按 Sink 计数。
    pub fn failures(&self) -> usize {
        self.failures.load(Ordering::Relaxed)
    }

    fn dispatch(&self, mut output: impl FnMut(&dyn Sink) -> fmt::Result) {
        for slot in &self.slots {
            let result = match slot.try_read() {
                Some(entry) => entry.sink.map_or(Ok(()), &mut output),
                None => Err(fmt::Error),
            };
            if result.is_err() {
                self.failures.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() as usize <= self.max_level.load(Ordering::Relaxed)
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
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
