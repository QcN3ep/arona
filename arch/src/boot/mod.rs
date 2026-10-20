// SPDX-License-Identifier: MPL-2.0

//! 固件启动信息解析与内核交接准备。

#[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
pub(crate) mod console;
pub mod dtb;

use kernel::{BootInfo, HardwareInfo, MemoryRegionState, PhysRange};

#[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
unsafe extern "C" {
    static _kernel_image: [u64; 2];
}

#[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
pub(crate) fn kernel_image() -> PhysRange {
    // SAFETY: _kernel_image 须是链接脚本提供的只读物理起止地址对。
    let [start, end] = unsafe { _kernel_image };
    PhysRange { start, end }
}

pub fn prepare_boot_info(
    boot_cpu_id: u64,
    kernel_image: PhysRange,
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
        .reserve(kernel_image, MemoryRegionState::Reserved)?;
    Ok(BootInfo {
        boot_cpu_id,
        kernel_image,
        hardware,
    })
}
