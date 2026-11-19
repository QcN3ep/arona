// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Trusted coherent DMA for the fixed QEMU platform.

// TODO: Implement IOMMU isolation.

use kernel::{
    DmaCapability, HardwareDeviceInfo, HardwareDeviceLocation, hardware::dma::Dma,
    memory::PhysRange,
};

crate::declare_hardware!(static DMA: QemuDma = QemuDma => kernel::hardware::dma::bind);

struct QemuDma;

impl Dma for QemuDma {
    fn address_bits(&self, device: &HardwareDeviceInfo) -> Option<u8> {
        if !cfg!(any(target_arch = "riscv64", target_arch = "loongarch64"))
            || !matches!(
                device.location,
                HardwareDeviceLocation::Pci { .. } | HardwareDeviceLocation::Platform
            )
        {
            return None;
        }
        let DmaCapability::Present(info) = device.dma else {
            return None;
        };
        (info.address_bits != 0 && info.address_bits <= 64 && info.isolation.is_none())
            .then_some(info.address_bits)
    }

    fn bus_address(&self, device: &HardwareDeviceInfo, range: PhysRange) -> Option<u64> {
        let bits = self.address_bits(device)?;
        // The kernel pins owned RAM frames before requesting their bus addresses.
        // The supported bus windows have no DMA translation.
        (range.start < range.end && (bits == 64 || range.end - 1 < (1u64 << bits)))
            .then_some(range.start)
    }

    fn synchronize(&self) {
        super::io_fence();
    }
}
