// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use core::arch::{global_asm, naked_asm};

#[unsafe(naked)]
pub extern "C" fn source() -> usize {
    naked_asm!(
        "addi.d $sp, $sp, -16",
        "st.d $ra, $sp, 0",
        ".global __arona_perf_source_start",
        ".set __arona_perf_source_start, .",
        "nop",
        "bl {body}",
        ".global __arona_perf_source_end",
        ".set __arona_perf_source_end, .",
        "nop",
        "ld.d $ra, $sp, 0",
        "addi.d $sp, $sp, 16",
        "jr $ra",
        body = sym super::source_body,
    );
}

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
    // The XOR changes the callee's add-immediate operand from one to two.
    naked_asm!(
        "la.abs $t5, __arona_perf_retranslated",
        "ld.w $t3, $t5, 0",
        "xori $t3, $t3, 3 << 10",
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
        "xori $t3, $t3, 3 << 10",
        "st.w $t3, $t5, 0",
        "ibar 0",
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

pub fn root(space: &super::mappings::Space) -> usize {
    physical(&space.tables[0] as *const _ as usize)
}

fn physical(address: usize) -> usize {
    address - 0xffff_8000_0000_0000
}

pub fn map_space(space: &mut super::mappings::Space) {
    for level in 0..3 {
        space.tables[level].0[0] = physical(&space.tables[level + 1] as *const _ as usize);
    }
    space.tables[3].0[super::mappings::CODE >> 12] =
        physical(&space.code as *const _ as usize) | 0x11;
    space.tables[3].0[super::mappings::DESCRIPTOR >> 12] =
        physical(&space.descriptor as *const _ as usize) | 0x4000_0000_0000_0011;
}

pub fn hide_descriptor(space: &mut super::mappings::Space) {
    space.tables[3].0[super::mappings::DESCRIPTOR >> 12] = 0;
}

pub fn switch_space(mut root: usize) {
    // SAFETY: PGDH retains the current kernel mapping; the new PGDL and its tables stay alive until switched away.
    unsafe {
        core::arch::asm!(
            "dbar 0", "csrwr {root}, 0x19", "csrwr $zero, 0x180",
            "invtlb 0, $zero, $zero", "ibar 0",
            root = inout(reg) root, options(nostack),
        );
    }
    let _ = root;
}

#[unsafe(naked)]
pub extern "C" fn mappings() -> usize {
    naked_asm!(
        "addi.d $sp, $sp, -16", "st.d $ra, $sp, 0",
        ".global __arona_perf_mappings_start", ".set __arona_perf_mappings_start, .", "nop",
        "bl {body}",
        ".global __arona_perf_mappings_end", ".set __arona_perf_mappings_end, .", "nop",
        "ld.d $ra, $sp, 0", "addi.d $sp, $sp, 16", "jr $ra",
        body = sym super::mappings::run,
    );
}

global_asm!(
    ".global __arona_perf_context_address",
    ".equ __arona_perf_context_address, 0x11000",
    ".pushsection .text.perf.images,\"ax\"",
    ".global __arona_perf_image_a",
    ".type __arona_perf_image_a,@function",
    "__arona_perf_image_a:",
    "nop",
    "addi.d $a0, $zero, 11",
    "jr $ra",
    ".size __arona_perf_image_a, .-__arona_perf_image_a",
    ".global __arona_perf_image_b",
    ".type __arona_perf_image_b,@function",
    "__arona_perf_image_b:",
    "nop",
    "addi.d $a0, $zero, 22",
    "jr $ra",
    ".size __arona_perf_image_b, .-__arona_perf_image_b",
    ".popsection",
);
