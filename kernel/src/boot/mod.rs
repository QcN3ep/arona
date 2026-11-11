// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Boot data definitions and kernel entry.

mod console;
mod hardware;
mod image;
mod memory;
mod program;

pub use console::{BootConsoleMapping, register_early_console};
pub use hardware::*;
pub use image::*;
pub use program::{BootData, BootProgram, ProgramSegment};

use crate::thread::scheduler;

#[derive(Clone, Copy, Debug)]
pub struct BootInfo<'boot> {
    pub boot_cpu_id: u64,
    pub kernel_image: KernelImage,
    pub early_console: Option<BootConsoleMapping>,
    pub hardware: &'boot HardwareInfo,
    pub init: Option<&'boot dyn BootProgram>,
    pub initramfs: Option<BootData>,
}

/// # Safety
///
/// `info` and its fields must be valid and consistent with the actual hardware; the current processor must be in the execution environment required to run the kernel.
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
    crate::hardware::interrupt_controller()
        .initialize(
            info.hardware,
            info.boot_cpu_id,
            &crate::hardware::mmio::DeviceMapper::new(info.hardware),
        )
        .unwrap_or_else(|error| panic!("failed to initialize interrupt controller: {error:?}"));
    log::info!("Interrupt controller initialized");
    crate::hardware::timer()
        .initialize(info.hardware.timebase_frequency_hz)
        .unwrap_or_else(|error| panic!("failed to initialize timer: {error:?}"));
    crate::logger::LOGGER.enable_timestamps(|| {
        let _interrupts = crate::hardware::cpu::InterruptGuard::new();
        crate::hardware::timer().now_ns().ok()
    });
    log::info!("Timer initialized");
    scheduler::initialize()
        .unwrap_or_else(|error| panic!("failed to initialize scheduler: {error:?}"));
    log::info!("Scheduler initialized");
    let init = info.init.expect("boot information is missing init");
    let initramfs = info
        .initramfs
        .expect("boot information is missing initramfs");
    let devices = crate::device::DeviceSet::initialize(info.hardware)
        .unwrap_or_else(|error| panic!("failed to initialize device resources: {error:?}"));
    program::start(init, initramfs, devices)
        .unwrap_or_else(|error| panic!("failed to start init: {error}"));
    console::detach().unwrap_or_else(|error| panic!("failed to detach early console: {error}"));
    log::debug!("Early console detached");
    scheduler::run()
}
