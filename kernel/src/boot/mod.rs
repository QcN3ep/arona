// SPDX-License-Identifier: MPL-2.0

//! 启动数据定义及内核入口。

mod hardware;
mod image;
mod memory;

pub use hardware::*;
pub use image::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootInfo<'boot> {
    pub boot_cpu_id: u64,
    pub kernel_image: KernelImage,
    pub early_console: Option<BootConsoleMapping>,
    pub hardware: &'boot HardwareInfo,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootConsoleMapping {
    pub virtual_address: usize,
    pub physical_address: u64,
    pub memory_type: crate::hardware::mmu::MemoryType,
}

/// # Safety
///
/// `info` 及其字段须合法且与实际硬件一致，当前处理器须处于内核运行所需的执行环境。
pub fn boot_kernel(info: BootInfo<'_>) -> ! {
    log::info!("Entering kernel");
    memory::initialize(&info)
        .unwrap_or_else(|error| panic!("failed to initialize memory: {error:?}"));
    log::info!("Memory initialized");
    let image = info.kernel_image;
    let stack = crate::memory::VirtRange {
        start: image.data.virtual_start
            + (image.boot_stack.start - image.data.physical.start) as usize,
        end: image.data.virtual_start + (image.boot_stack.end - image.data.physical.start) as usize,
    };
    crate::hardware::trap()
        .install(stack, crate::trap::dispatch)
        .unwrap_or_else(|error| panic!("failed to install trap entry: {error:?}"));
    log::info!("Trap initialized");
    loop {
        core::hint::spin_loop();
    }
}
