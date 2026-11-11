// SPDX-License-Identifier: MPL-2.0

use core::arch::naked_asm;

pub fn frequency(dtb: usize) -> Option<u64> {
    let pointer = (0xffff_ffc0_0040_0000 + (dtb & 0x1f_ffff)) as *const u32;
    // SAFETY: The firmware DTB must be fully mapped into the boot firmware window and remain unchanged during parsing.
    let tree = unsafe { fdt::Fdt::from_ptr_fallible(pointer) }.unwrap();
    tree.root()
        .unwrap()
        .cpus()
        .unwrap()
        .common_timebase_frequency()
        .unwrap()
}

#[unsafe(naked)]
pub extern "C" fn round_trip() -> usize {
    naked_asm!(
        "addi sp, sp, -16",
        "sd s0, 0(sp)",
        "li t0, 11",
        "li t1, 12",
        "li a0, 21",
        "li a1, 22",
        "li s0, 31",
        "wfi",
        "csrsi sstatus, 2",
        "csrci sstatus, 2",
        "li t2, 11",
        "bne t0, t2, 2f",
        "li t2, 12",
        "bne t1, t2, 2f",
        "li t2, 21",
        "bne a0, t2, 2f",
        "li t2, 22",
        "bne a1, t2, 2f",
        "li t2, 31",
        "bne s0, t2, 2f",
        "li a0, 0",
        "j 3f",
        "2:",
        "li a0, 1",
        "3:",
        "ld s0, 0(sp)",
        "addi sp, sp, 16",
        "ret",
    );
}
