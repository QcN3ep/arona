// SPDX-License-Identifier: MPL-2.0

//! 启动数据定义及内核入口。

mod hardware;

pub use hardware::{
    CpuInfo, DeviceUsePolicy, DmaInfo, DmaIsolationRoute, HardwareDeviceInfo,
    HardwareDeviceInstanceId, HardwareDeviceKind, HardwareDeviceLocation, HardwareInfo,
    HardwareInterruptInfo, InterruptPolarity, InterruptTrigger, MAX_CPUS, MAX_DEVICES,
    MAX_INTERRUPTS, MAX_MEMORY_REGIONS, MAX_MMIO_REGIONS, MAX_PCI_MEMORY_WINDOWS, MemoryInfo,
    MemoryRegion, MemoryRegionState, PhysRange,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootInfo<'boot> {
    pub boot_cpu_id: u64,
    pub kernel_image: PhysRange,
    pub hardware: &'boot HardwareInfo,
}

impl BootInfo<'_> {
    pub fn validate(&self) {
        self.hardware
            .validate_resources()
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(
            self.hardware
                .cpus
                .iter()
                .any(|cpu| cpu.hardware_id == self.boot_cpu_id),
            "boot CPU is missing from the hardware description"
        );
        assert!(
            self.hardware.memory.is_mappable(self.kernel_image),
            "kernel image is not fully contained in mappable RAM"
        );
    }
}

pub fn boot_kernel(info: BootInfo<'_>) -> ! {
    info.validate();
    loop {
        core::hint::spin_loop();
    }
}
