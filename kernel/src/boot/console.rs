// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Handoff of the boot console mapping and log Sink.

use crate::logger::{LOGGER, Sink, SinkError, SinkHandle};
use spin::Mutex;

static EARLY_CONSOLE: Mutex<Option<SinkHandle<'static>>> = Mutex::new(None);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootConsoleMapping {
    pub virtual_address: usize,
    pub physical_address: u64,
    pub memory_type: crate::hardware::mmu::MemoryType,
}

pub fn register_early_console(sink: &'static dyn Sink) -> Result<(), SinkError> {
    let mut handle = EARLY_CONSOLE.try_lock().ok_or(SinkError::Busy)?;
    if handle.is_some() {
        return Err(SinkError::Full);
    }
    *handle = Some(LOGGER.add_sink(sink)?);
    Ok(())
}

pub(super) fn detach() -> Result<(), SinkError> {
    let mut handle = EARLY_CONSOLE.try_lock().ok_or(SinkError::Busy)?;
    if let Some(current) = handle.as_ref() {
        current.remove()?;
        *handle = None;
    }
    Ok(())
}
