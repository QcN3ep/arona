// SPDX-License-Identifier: MPL-2.0

//! 启动数据定义及内核入口。

mod hardware;
mod image;

pub use hardware::*;
pub use image::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootInfo<'boot> {
    pub boot_cpu_id: u64,
    pub kernel_image: KernelImage,
    pub hardware: &'boot HardwareInfo,
}

/// # Safety
///
/// `info` 及其字段须合法且与实际硬件一致，当前处理器须处于内核运行所需的执行环境。
pub fn boot_kernel(info: BootInfo<'_>) -> ! {
    log::info!("Entering kernel");
    loop {
        core::hint::spin_loop();
    }
}
