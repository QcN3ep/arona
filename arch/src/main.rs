// SPDX-License-Identifier: MPL-2.0

#![no_std]
#![no_main]

#[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
compile_error!("Arona only supports RISC-V 64 and LoongArch 64");

#[cfg(target_arch = "loongarch64")]
use arch::loongarch64::boot;
#[cfg(target_arch = "riscv64")]
use arch::riscv64::boot;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    boot::panic(info)
}
