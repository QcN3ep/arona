// SPDX-License-Identifier: MPL-2.0

use core::marker::PhantomData;

use super::{BindError, Slot, mmio::MmioMapper};
use crate::{HardwareDeviceInstanceId, HardwareInfo, HardwareInterruptInfo};

static SLOT: Slot<&'static dyn InterruptController> = Slot::new();

pub fn bind(controller: &'static dyn InterruptController) -> Result<(), BindError> {
    SLOT.bind(controller)
}

pub fn interrupt_controller() -> &'static dyn InterruptController {
    SLOT.get()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterruptError {
    NotInitialized,
    AlreadyInitialized,
    InterruptsEnabled,
    Busy,
    InvalidSource,
    InvalidTopology,
    InvalidClaim,
    Unsupported,
    Memory(crate::memory::MemoryError),
}

impl From<crate::memory::MemoryError> for InterruptError {
    fn from(error: crate::memory::MemoryError) -> Self {
        Self::Memory(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InterruptSource {
    pub controller: HardwareDeviceInstanceId,
    pub line: u32,
}

impl From<HardwareInterruptInfo> for InterruptSource {
    fn from(info: HardwareInterruptInfo) -> Self {
        Self {
            controller: info.controller,
            line: info.line,
        }
    }
}

type Completion = fn(InterruptSource, usize, bool) -> Result<(), InterruptError>;

#[must_use]
#[derive(Debug)]
pub struct InterruptClaim {
    source: InterruptSource,
    token: usize,
    completion: Option<Completion>,
    local: PhantomData<*mut ()>,
}

impl InterruptClaim {
    pub fn new(source: InterruptSource, token: usize, completion: Completion) -> Self {
        Self {
            source,
            token,
            completion: Some(completion),
            local: PhantomData,
        }
    }

    pub fn source(&self) -> InterruptSource {
        self.source
    }

    pub fn complete(mut self) -> Result<(), InterruptError> {
        if let Some(complete) = self.completion {
            complete(self.source, self.token, false)?;
            self.completion = None;
        }
        Ok(())
    }
}

impl Drop for InterruptClaim {
    fn drop(&mut self) {
        // Mask an unhandled or failed input first, then complete the claim.
        if let Some(complete) = self.completion.take()
            && let Err(error) = complete(self.source, self.token, true)
        {
            log::error!("Failed to complete external interrupt: {error:?}");
            super::cpu().halt();
        }
    }
}

/// # Safety
///
/// Operations and drop of claimed objects must run on the initialization processor with interrupts disabled, and the exception entry and its stack must be ready.
pub trait InterruptController: Sync {
    /// Initializes the routing and masks all inputs; the hardware description must match the current platform.
    fn initialize(
        &self,
        hardware: &HardwareInfo,
        cpu_id: u64,
        mmio: &dyn MmioMapper,
    ) -> Result<(), InterruptError>;

    fn mask(&self, source: InterruptSource) -> Result<(), InterruptError>;

    /// Unmasks an input after setting its trigger mode; the input must not be reconfigured while it is claimed.
    fn unmask(&self, interrupt: HardwareInterruptInfo) -> Result<(), InterruptError>;

    /// At most one claim may be outstanding; returns None when no input is pending.
    fn claim(&self) -> Result<Option<InterruptClaim>, InterruptError>;
}
