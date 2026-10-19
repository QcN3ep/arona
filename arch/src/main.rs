// SPDX-License-Identifier: MPL-2.0

#![no_std]
#![no_main]

#[cfg(not(target_arch = "riscv64"))]
compile_error!("Arona only supports riscv64gc-unknown-none-elf");

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    #[cfg(target_arch = "riscv64")]
    arch::riscv64::boot::halt()
}
