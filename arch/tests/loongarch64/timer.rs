// SPDX-License-Identifier: MPL-2.0

use core::arch::naked_asm;

pub fn frequency(_firmware: usize) -> Option<u64> {
    None
}

#[unsafe(naked)]
pub extern "C" fn round_trip() -> usize {
    naked_asm!(
        "addi.d $sp, $sp, -16",
        "st.d $s0, $sp, 0",
        "addi.d $t0, $zero, 11",
        "addi.d $t1, $zero, 12",
        "addi.d $a0, $zero, 21",
        "addi.d $a1, $zero, 22",
        "addi.d $s0, $zero, 31",
        "idle 0",
        "addi.d $t2, $zero, 4",
        "csrxchg $t2, $t2, 0x0",
        "addi.d $t2, $zero, 4",
        "csrxchg $zero, $t2, 0x0",
        "addi.d $t2, $zero, 11",
        "bne $t0, $t2, 2f",
        "addi.d $t2, $zero, 12",
        "bne $t1, $t2, 2f",
        "addi.d $t2, $zero, 21",
        "bne $a0, $t2, 2f",
        "addi.d $t2, $zero, 22",
        "bne $a1, $t2, 2f",
        "addi.d $t2, $zero, 31",
        "bne $s0, $t2, 2f",
        "move $a0, $zero",
        "b 3f",
        "2:",
        "addi.d $a0, $zero, 1",
        "3:",
        "ld.d $s0, $sp, 0",
        "addi.d $sp, $sp, 16",
        "jirl $zero, $ra, 0",
    );
}
