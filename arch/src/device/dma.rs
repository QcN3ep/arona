// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Trusted coherent DMA for the fixed QEMU platform.

// TODO: Implement IOMMU isolation and determine the DMA address range from the actual RAM and device topology.

use kernel::{
    HardwareDeviceInfo, HardwareDeviceLocation,
    hardware::dma::{Dma, DmaProfile},
    memory::PhysRange,
};

crate::declare_hardware!(static DMA: QemuDma = QemuDma => kernel::hardware::dma::bind);

struct QemuDma;

impl Dma for QemuDma {
    fn profile(&self) -> DmaProfile {
        DmaProfile::CompetitionTrusted
    }

    fn address_bits(&self, device: &HardwareDeviceInfo) -> Option<u8> {
        if !cfg!(any(target_arch = "riscv64", target_arch = "loongarch64")) {
            return None;
        }
        let supported = match &device.location {
            HardwareDeviceLocation::Pci { .. } => {
                // Modern 0x1040-series and transitional legacy VirtIO PCI IDs.
                device.has_identity("pci", "1af4:1041")
                    || device.has_identity("pci", "1af4:1042")
                    || device.has_identity("pci", "1af4:1044")
                    || device.has_identity("pci", "1af4:1000")
                    || device.has_identity("pci", "1af4:1001")
                    || device.has_identity("pci", "1af4:1005")
            }
            HardwareDeviceLocation::Platform => {
                cfg!(target_arch = "riscv64")
                    && device.has_identity("devicetree", "virtio,mmio")
                    && device.mmio_regions.len() == 1
                    && device.mmio_regions[0].start >= 0x1000_1000
                    && device.mmio_regions[0].end <= 0x1000_9000
                    && device.mmio_regions[0].start.is_multiple_of(4096)
                    && device.mmio_regions[0].end - device.mmio_regions[0].start == 4096
            }
            _ => false,
        };
        supported.then_some(64)
    }

    fn bus_address(&self, device: &HardwareDeviceInfo, range: PhysRange) -> Option<u64> {
        self.address_bits(device)?;
        let ram = if cfg!(target_arch = "riscv64") {
            0x8000_0000..0x9000_0000
        } else {
            0..0x1000_0000
        };
        (range.start >= ram.start && range.start < range.end && range.end <= ram.end)
            .then_some(range.start)
    }

    fn synchronize(&self) {
        super::io_fence();
    }
}
