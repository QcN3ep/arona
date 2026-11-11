// SPDX-License-Identifier: MPL-2.0

//! DMA addressing, trust configuration, and device memory synchronization capability.

use super::{BindError, Slot};
use crate::{HardwareDeviceInfo, memory::PhysRange};
pub use abi::dma::DmaProfile;

static SLOT: Slot<&'static dyn Dma> = Slot::new();

pub fn bind(provider: &'static dyn Dma) -> Result<(), BindError> {
    SLOT.bind(provider)
}

pub fn dma() -> &'static dyn Dma {
    SLOT.get()
}

pub trait Dma: Sync {
    fn profile(&self) -> DmaProfile;
    fn address_bits(&self, device: &HardwareDeviceInfo) -> Option<u8>;
    fn bus_address(&self, device: &HardwareDeviceInfo, range: PhysRange) -> Option<u64>;
    fn synchronize(&self);
}
