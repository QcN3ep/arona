// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use core::arch::{global_asm, naked_asm};

#[unsafe(naked)]
pub extern "C" fn empty() -> usize {
    naked_asm!(
        ".option push",
        ".option norvc",
        ".option norelax",
        "li a0, 0",
        ".global __arona_perf_empty_start",
        ".set __arona_perf_empty_start, .",
        "nop",
        ".global __arona_perf_empty_end",
        ".set __arona_perf_empty_end, .",
        "nop",
        "ret",
        ".global __arona_perf_empty_expected",
        ".equ __arona_perf_empty_expected, 0",
        ".option pop",
    );
}

#[unsafe(naked)]
pub extern "C" fn straight() -> usize {
    naked_asm!(
        ".option push",
        ".option norvc",
        ".option norelax",
        "li a0, 0",
        ".global __arona_perf_straight_start",
        ".set __arona_perf_straight_start, .",
        "nop",
        "addi a0, a0, 1",
        "addi a0, a0, 1",
        "addi a0, a0, 1",
        "addi a0, a0, 1",
        ".global __arona_perf_straight_end",
        ".set __arona_perf_straight_end, .",
        "nop",
        "ret",
        ".global __arona_perf_straight_expected",
        ".equ __arona_perf_straight_expected, 4",
        ".option pop",
    );
}

#[unsafe(naked)]
pub extern "C" fn loops() -> usize {
    naked_asm!(
        ".option push",
        ".option norvc",
        ".option norelax",
        "li a0, 0",
        ".global __arona_perf_loop_start",
        ".set __arona_perf_loop_start, .",
        "nop",
        "li t0, 16",
        "2: addi a0, a0, 1",
        "addi t0, t0, -1",
        "bnez t0, 2b",
        ".global __arona_perf_loop_end",
        ".set __arona_perf_loop_end, .",
        "nop",
        "ret",
        ".global __arona_perf_loop_expected",
        ".equ __arona_perf_loop_expected, 49",
        ".option pop",
    );
}

#[unsafe(naked)]
pub extern "C" fn call() -> usize {
    naked_asm!(
        ".option push",
        ".option norvc",
        ".option norelax",
        "li a0, 0",
        ".global __arona_perf_call_start",
        ".set __arona_perf_call_start, .",
        "nop",
        "jal t5, 2f",
        ".global __arona_perf_call_end",
        ".set __arona_perf_call_end, .",
        "nop",
        "ret",
        "2: addi a0, a0, 1",
        "jr t5",
        ".global __arona_perf_call_expected",
        ".equ __arona_perf_call_expected, 3",
        ".option pop",
    );
}

#[unsafe(naked)]
pub extern "C" fn retry() -> usize {
    naked_asm!(
        ".option push",
        ".option norvc",
        ".option norelax",
        "addi sp, sp, -16",
        "li t0, 73",
        "sd t0, 0(sp)",
        "csrr t6, stvec",
        "lla t0, 2f",
        "csrw stvec, t0",
        "li t3, 0",
        "li a1, 0",
        ".global __arona_perf_retry_start",
        ".set __arona_perf_retry_start, .",
        "nop",
        "ld a0, 0(t3)",
        "addi a0, a0, 1",
        ".global __arona_perf_retry_end",
        ".set __arona_perf_retry_end, .",
        "nop",
        "csrw stvec, t6",
        "add a0, a0, a1",
        "addi sp, sp, 16",
        "ret",
        ".balign 4",
        "2: mv t3, sp",
        "addi a1, a1, 1",
        "sret",
        ".global __arona_perf_retry_expected",
        ".equ __arona_perf_retry_expected, 6",
        ".option pop",
    );
}

#[unsafe(naked)]
pub extern "C" fn interrupt() -> usize {
    naked_asm!(
        ".option push",
        ".option norvc",
        ".option norelax",
        "csrr t6, stvec",
        "csrr t5, sie",
        "lla t0, 2f",
        "csrw stvec, t0",
        "csrwi sie, 2",
        "li a0, 0",
        ".global __arona_perf_interrupt_start",
        ".set __arona_perf_interrupt_start, .",
        "nop",
        "csrsi sip, 2",
        "csrsi sstatus, 2",
        "csrci sstatus, 2",
        ".global __arona_perf_interrupt_end",
        ".set __arona_perf_interrupt_end, .",
        "nop",
        "csrw sie, t5",
        "csrw stvec, t6",
        "ret",
        ".balign 4",
        "2: csrci sip, 2",
        "addi a0, a0, 1",
        "sret",
        ".global __arona_perf_interrupt_expected",
        ".equ __arona_perf_interrupt_expected, 6",
        ".option pop",
    );
}

#[unsafe(naked)]
pub extern "C" fn retranslation() -> usize {
    naked_asm!(
        ".option push",
        ".option norvc",
        ".option norelax",
        "lla t4, __arona_perf_retranslated",
        "lw t3, 0(t4)",
        "li a0, 0",
        ".global __arona_perf_retranslation_start",
        ".set __arona_perf_retranslation_start, .",
        "nop",
        "jal t5, __arona_perf_retranslated",
        "sw t3, 0(t4)",
        "fence.i",
        "jal t5, __arona_perf_retranslated",
        ".global __arona_perf_retranslation_end",
        ".set __arona_perf_retranslation_end, .",
        "nop",
        "ret",
        ".global __arona_perf_retranslation_expected",
        ".equ __arona_perf_retranslation_expected, 8",
        ".option pop",
    );
}

// The separate page lets the store invalidate the callee without invalidating the writer's block.
global_asm!(
    ".pushsection .text.perf.callee,\"ax\"",
    ".balign 4096",
    ".option push",
    ".option norvc",
    ".option norelax",
    ".global __arona_perf_retranslated",
    "__arona_perf_retranslated:",
    "addi a0, a0, 1",
    "jr t5",
    ".option pop",
    ".popsection",
);
