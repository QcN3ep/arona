// SPDX-License-Identifier: MPL-2.0

//! 跨架构设备驱动。

pub(crate) mod dma;
pub(crate) mod pci;
pub mod uart;

#[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
pub(crate) mod mmio;

fn io_fence() {
    #[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
    // SAFETY: 汇编须使用目标 ISA 的屏障指令，且不得修改栈或通用寄存器。
    unsafe {
        #[cfg(target_arch = "riscv64")]
        core::arch::asm!("fence iorw, iorw", options(nostack, preserves_flags));
        #[cfg(target_arch = "loongarch64")]
        core::arch::asm!("dbar 0", options(nostack, preserves_flags));
    }
}
