// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Firmware boot information parsing and kernel handoff preparation.

// TODO: Support multiprocessor boot and firmware handoff on real hardware platforms.

#[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
pub(crate) mod console;
pub mod dtb;
#[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
mod image;
pub mod initramfs;

use kernel::{BootInfo, HardwareInfo, KernelImage, memory::MemoryRegionState};

#[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
pub use image::kernel_image;

pub fn prepare_boot_info(
    boot_cpu_id: u64,
    kernel_image: KernelImage,
    hardware: &mut HardwareInfo,
) -> Result<BootInfo<'_>, &'static str> {
    if !hardware
        .cpus
        .iter()
        .any(|cpu| cpu.hardware_id == boot_cpu_id)
    {
        return Err("boot CPU is missing from the hardware description");
    }
    hardware
        .memory
        .reserve(kernel_image.physical, MemoryRegionState::Reserved)?;
    Ok(BootInfo {
        boot_cpu_id,
        kernel_image,
        early_console: None,
        hardware,
        init: None,
        initramfs: None,
    })
}
