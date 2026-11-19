// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Handoff of the boot console mapping and log Sink.

use crate::logger::{LOGGER, Sink, SinkError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootConsoleMapping {
    pub virtual_address: usize,
    pub physical_address: u64,
    pub memory_type: crate::hardware::mmu::MemoryType,
}

pub fn register_early_console(sink: &'static dyn Sink) -> Result<(), SinkError> {
    LOGGER.set_sink(sink)
}

pub(super) fn detach() -> Result<(), SinkError> {
    LOGGER.remove_sink()
}
