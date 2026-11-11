// SPDX-License-Identifier: MPL-2.0

//! Cross-architecture device drivers.

pub(crate) mod dma;
pub(crate) mod pci;
pub mod uart;

#[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
pub(crate) mod mmio;

fn io_fence() {
    #[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
    // SAFETY: The assembly must use the target ISA's barrier instruction and must not modify the stack or general-purpose registers.
    unsafe {
        #[cfg(target_arch = "riscv64")]
        core::arch::asm!("fence iorw, iorw", options(nostack, preserves_flags));
        #[cfg(target_arch = "loongarch64")]
        core::arch::asm!("dbar 0", options(nostack, preserves_flags));
    }
}
