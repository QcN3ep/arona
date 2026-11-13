// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use core::arch::{global_asm, naked_asm};

#[unsafe(naked)]
pub extern "C" fn empty() -> usize {
    naked_asm!(
        "move $a0, $zero",
        ".global __arona_perf_empty_start",
        ".set __arona_perf_empty_start, .",
        "nop",
        ".global __arona_perf_empty_end",
        ".set __arona_perf_empty_end, .",
        "nop",
        "jr $ra",
        ".global __arona_perf_empty_expected",
        ".equ __arona_perf_empty_expected, 0",
    );
}

#[unsafe(naked)]
pub extern "C" fn straight() -> usize {
    naked_asm!(
        "move $a0, $zero",
        ".global __arona_perf_straight_start",
        ".set __arona_perf_straight_start, .",
        "nop",
        "addi.d $a0, $a0, 1",
        "addi.d $a0, $a0, 1",
        "addi.d $a0, $a0, 1",
        "addi.d $a0, $a0, 1",
        ".global __arona_perf_straight_end",
        ".set __arona_perf_straight_end, .",
        "nop",
        "jr $ra",
        ".global __arona_perf_straight_expected",
        ".equ __arona_perf_straight_expected, 4",
    );
}

#[unsafe(naked)]
pub extern "C" fn loops() -> usize {
    naked_asm!(
        "move $a0, $zero",
        ".global __arona_perf_loop_start",
        ".set __arona_perf_loop_start, .",
        "nop",
        "addi.d $t0, $zero, 16",
        "2: addi.d $a0, $a0, 1",
        "addi.d $t0, $t0, -1",
        "bnez $t0, 2b",
        ".global __arona_perf_loop_end",
        ".set __arona_perf_loop_end, .",
        "nop",
        "jr $ra",
        ".global __arona_perf_loop_expected",
        ".equ __arona_perf_loop_expected, 49",
    );
}

#[unsafe(naked)]
pub extern "C" fn call() -> usize {
    naked_asm!(
        "la.abs $t5, 2f",
        "move $a0, $zero",
        ".global __arona_perf_call_start",
        ".set __arona_perf_call_start, .",
        "nop",
        "jirl $t6, $t5, 0",
        ".global __arona_perf_call_end",
        ".set __arona_perf_call_end, .",
        "nop",
        "jr $ra",
        "2: addi.d $a0, $a0, 1",
        "jr $t6",
        ".global __arona_perf_call_expected",
        ".equ __arona_perf_call_expected, 3",
    );
}

#[unsafe(naked)]
pub extern "C" fn retry() -> usize {
    naked_asm!(
        "la.abs $t6, 2f",
        "csrwr $t6, 0xc",
        "move $t3, $zero",
        "move $a1, $zero",
        "addi.d $a0, $zero, 73",
        ".global __arona_perf_retry_start",
        ".set __arona_perf_retry_start, .",
        "nop",
        "asrtgt.d $t3, $zero",
        "addi.d $a0, $a0, 1",
        ".global __arona_perf_retry_end",
        ".set __arona_perf_retry_end, .",
        "nop",
        "csrwr $t6, 0xc",
        "add.d $a0, $a0, $a1",
        "jr $ra",
        ".balign 64",
        "2: addi.d $t3, $zero, 1",
        "addi.d $a1, $a1, 1",
        "ertn",
        ".global __arona_perf_retry_expected",
        ".equ __arona_perf_retry_expected, 6",
    );
}

#[unsafe(naked)]
pub extern "C" fn interrupt() -> usize {
    naked_asm!(
        "la.abs $t6, 2f",
        "csrwr $t6, 0xc",
        "addi.d $t7, $zero, 1",
        "csrwr $t7, 0x4",
        "move $a0, $zero",
        ".global __arona_perf_interrupt_start",
        ".set __arona_perf_interrupt_start, .",
        "nop",
        "addi.d $t0, $zero, 1",
        "csrwr $t0, 0x5",
        "addi.d $t0, $zero, 4",
        "csrxchg $t0, $t0, 0x0",
        "addi.d $t0, $zero, 4",
        "csrxchg $zero, $t0, 0x0",
        ".global __arona_perf_interrupt_end",
        ".set __arona_perf_interrupt_end, .",
        "nop",
        "csrwr $t7, 0x4",
        "csrwr $t6, 0xc",
        "jr $ra",
        ".balign 64",
        "2: csrwr $zero, 0x5",
        "addi.d $a0, $a0, 1",
        "ertn",
        ".global __arona_perf_interrupt_expected",
        ".equ __arona_perf_interrupt_expected, 9",
    );
}

#[unsafe(naked)]
pub extern "C" fn retranslation() -> usize {
    naked_asm!(
        "la.abs $t5, __arona_perf_retranslated",
        "ld.w $t3, $t5, 0",
        "move $a0, $zero",
        ".global __arona_perf_retranslation_start",
        ".set __arona_perf_retranslation_start, .",
        "nop",
        "jirl $t6, $t5, 0",
        "st.w $t3, $t5, 0",
        "ibar 0",
        "jirl $t6, $t5, 0",
        ".global __arona_perf_retranslation_end",
        ".set __arona_perf_retranslation_end, .",
        "nop",
        "jr $ra",
        ".global __arona_perf_retranslation_expected",
        ".equ __arona_perf_retranslation_expected, 8",
    );
}

// The separate page lets the store invalidate the callee without invalidating the writer's block.
global_asm!(
    ".pushsection .text.perf.callee,\"ax\"",
    ".balign 4096",
    ".global __arona_perf_retranslated",
    "__arona_perf_retranslated:",
    "addi.d $a0, $a0, 1",
    "jr $t6",
    ".popsection",
);
