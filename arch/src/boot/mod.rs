// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Firmware boot information parsing and kernel handoff preparation.

// TODO: Support multiprocessor boot and firmware handoff on real hardware platforms.

#[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
pub(crate) mod console;
pub mod dtb;
#[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
mod image;
#[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
pub(crate) mod initramfs;

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

#[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
pub(crate) fn enter(
    boot_cpu_id: u64,
    hardware: &mut HardwareInfo,
    external_initramfs: Option<kernel::memory::PhysRange>,
    machine: u16,
    console: kernel::BootConsoleMapping,
) -> ! {
    crate::hardware::bind();
    let image = kernel_image();
    let archive = initramfs::source(external_initramfs, image)
        .unwrap_or_else(|error| panic!("failed to locate initramfs: {error}"));
    let init = kernel::boot::initramfs::prepare(archive, hardware, image, machine)
        .unwrap_or_else(|error| panic!("failed to parse init image: {error}"));
    let mut info = prepare_boot_info(boot_cpu_id, image, hardware)
        .unwrap_or_else(|error| panic!("failed to prepare boot information: {error}"));
    info.init = Some(&init);
    info.initramfs = Some(archive.data());
    info.early_console = Some(console);
    // SAFETY: The architecture handoff has initialized hardware and reserved firmware storage, and the local archive and ELF outlive this non-returning call.
    kernel::boot_kernel(info)
}

#[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
pub(crate) fn clear_bss() {
    unsafe extern "C" {
        static _sbss: u8;
        static _ebss: u8;
    }
    let mut cursor = core::ptr::addr_of!(_sbss) as usize;
    let end = core::ptr::addr_of!(_ebss) as usize;
    while cursor < end {
        // SAFETY: Boot invokes this before using BSS; the linker supplies an exclusive, writable, word-aligned region.
        unsafe {
            (cursor as *mut u64).write_volatile(0);
        }
        cursor += size_of::<u64>();
    }
}
