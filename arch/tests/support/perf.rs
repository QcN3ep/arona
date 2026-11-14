// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Instruction boundaries shared by every performance case in a test image.

#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn __arona_kperf_start() {
    #[cfg(target_arch = "riscv64")]
    core::arch::naked_asm!("nop", "ret");
    #[cfg(target_arch = "loongarch64")]
    core::arch::naked_asm!("nop", "jr $ra");
}

#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn __arona_kperf_end() {
    #[cfg(target_arch = "riscv64")]
    core::arch::naked_asm!("nop", "nop", "ret");
    #[cfg(target_arch = "loongarch64")]
    core::arch::naked_asm!("nop", "nop", "jr $ra");
}

pub use __arona_kperf_end as end;
pub use __arona_kperf_start as start;
