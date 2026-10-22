// SPDX-License-Identifier: MPL-2.0

use kernel::hardware::{
    self,
    cpu::InterruptState,
    interrupt::{InterruptError, InterruptSource},
};
use spin::{Mutex, MutexGuard};

impl From<crate::device::mmio::MmioError> for InterruptError {
    fn from(error: crate::device::mmio::MmioError) -> Self {
        match error {
            crate::device::mmio::MmioError::InvalidRange => Self::InvalidTopology,
        }
    }
}

pub(crate) struct LocalController<T> {
    state: Mutex<Option<T>>,
}

impl<T> LocalController<T> {
    pub const fn new() -> Self {
        Self {
            state: Mutex::new(None),
        }
    }

    pub fn initialize(
        &self,
        create: impl FnOnce() -> Result<T, InterruptError>,
    ) -> Result<(), InterruptError> {
        let mut slot = self.try_lock()?;
        if slot.is_some() {
            return Err(InterruptError::AlreadyInitialized);
        }
        *slot = Some(create()?);
        Ok(())
    }

    pub fn with<R>(
        &self,
        operation: impl FnOnce(&mut T) -> Result<R, InterruptError>,
    ) -> Result<R, InterruptError> {
        let mut slot = self.try_lock()?;
        operation(slot.as_mut().ok_or(InterruptError::NotInitialized)?)
    }

    fn try_lock(&self) -> Result<MutexGuard<'_, Option<T>>, InterruptError> {
        if hardware::cpu().interrupt_state() != InterruptState::Disabled {
            return Err(InterruptError::InterruptsEnabled);
        }
        self.state.try_lock().ok_or(InterruptError::Busy)
    }
}

#[derive(Default)]
pub(crate) struct Claims {
    sequence: usize,
    active: Option<(InterruptSource, usize)>,
}

impl Claims {
    pub fn available(&self) -> Result<(), InterruptError> {
        if self.active.is_some() || self.sequence == usize::MAX {
            return Err(InterruptError::Busy);
        }
        Ok(())
    }

    pub fn begin(&mut self, source: InterruptSource) -> usize {
        self.sequence += 1;
        self.active = Some((source, self.sequence));
        self.sequence
    }

    pub fn contains(&self, source: InterruptSource) -> bool {
        self.active.is_some_and(|(active, _)| active == source)
    }

    pub fn validate(&self, source: InterruptSource, token: usize) -> Result<(), InterruptError> {
        if self.active != Some((source, token)) {
            return Err(InterruptError::InvalidClaim);
        }
        Ok(())
    }

    pub fn finish(&mut self) {
        self.active = None;
    }
}
