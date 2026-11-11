// SPDX-License-Identifier: MPL-2.0

//! Userspace log client, management interface, and log service.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

mod client;
mod protocol;
pub mod service;

pub use client::{Client, Control};
pub use log;
pub use runtime::logger::{Level, MAX_RECORD_SIZE, Record};

use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Once;

const FLUSH_TIMEOUT_NS: u64 = 10_000_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Runtime(runtime::Error),
    Call(runtime::ipc::CallError),
    Device(devmgr::Error),
    InvalidRequest,
    BufferTooSmall,
    Unavailable,
    Busy,
}

impl From<runtime::Error> for Error {
    fn from(error: runtime::Error) -> Self {
        Self::Runtime(error)
    }
}

impl From<devmgr::Error> for Error {
    fn from(error: devmgr::Error) -> Self {
        Self::Device(error)
    }
}

struct Facade {
    client: Once<Client<'static>>,
    failures: AtomicUsize,
}

static FACADE: Facade = Facade {
    client: Once::new(),
    failures: AtomicUsize::new(0),
};

pub fn init(client: Client<'static>, level: log::LevelFilter) -> Result<(), log::SetLoggerError> {
    FACADE.client.call_once(|| client);
    log::set_logger(&FACADE)?;
    log::set_max_level(level);
    Ok(())
}

pub fn failures() -> usize {
    FACADE.failures.load(Ordering::Relaxed)
}

impl log::Log for Facade {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata())
            && let Some(client) = self.client.get()
            && client
                .try_write_fmt(record.level(), record.target(), *record.args())
                .is_err()
        {
            self.failures.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn flush(&self) {
        if let Some(client) = self.client.get() {
            let deadline = runtime::thread::now_ns()
                .ok()
                .and_then(|now| now.checked_add(FLUSH_TIMEOUT_NS));
            if deadline.is_none_or(|deadline| client.flush(deadline).is_err()) {
                self.failures.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}
