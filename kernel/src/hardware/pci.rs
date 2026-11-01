// SPDX-License-Identifier: MPL-2.0

//! PCI 配置空间访问能力。

use crate::memory::DeviceMapping;

use super::{BindError, Slot};

static SLOT: Slot<&'static dyn PciConfigAccess> = Slot::new();

pub fn bind(access: &'static dyn PciConfigAccess) -> Result<(), BindError> {
    SLOT.bind(access)
}

pub fn pci_config() -> &'static dyn PciConfigAccess {
    SLOT.get()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PciConfigError {
    InvalidOffset,
}

pub trait PciConfigAccess: Sync {
    /// 读取映射范围内对齐的 32 位配置寄存器。
    fn read32(&self, mapping: &DeviceMapping, offset: usize) -> Result<u32, PciConfigError>;

    /// 写入映射范围内对齐的 32 位配置寄存器。
    fn write32(
        &self,
        mapping: &DeviceMapping,
        offset: usize,
        value: u32,
    ) -> Result<(), PciConfigError>;
}
