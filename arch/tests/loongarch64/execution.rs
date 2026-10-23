// SPDX-License-Identifier: MPL-2.0

use core::arch::{asm, naked_asm};
use kernel::hardware::execution::Context;

pub fn frequency(_firmware: usize) -> Option<u64> {
    None
}

pub fn set_floating_enabled(enabled: bool) {
    // SAFETY: 当前处理器须允许修改 EUEN，修改期间不得执行浮点指令。
    unsafe {
        asm!("csrxchg {}, {}, 0x2", inout(reg) usize::from(enabled) => _, in(reg) 1usize, options(nomem, nostack));
    }
}

pub fn floating_enabled() -> bool {
    let extensions: usize;
    // SAFETY: 执行环境须允许读取 EUEN。
    unsafe {
        asm!("csrrd {}, 0x2", out(reg) extensions, options(nomem, nostack));
    }
    extensions & 1 != 0
}

#[unsafe(naked)]
pub extern "C" fn switch(_current: &Context, _next: &Context) -> usize {
    naked_asm!(
        "addi.d $sp, $sp, -48",
        "st.d $ra, $sp, 0",
        "st.d $fp, $sp, 8",
        "st.d $s0, $sp, 16",
        "st.d $s4, $sp, 24",
        "st.d $s8, $sp, 32",
        "addi.d $fp, $zero, 41",
        "addi.d $s0, $zero, 42",
        "addi.d $s4, $zero, 43",
        "addi.d $s8, $zero, 44",
        "bl {switch}",
        "addi.d $a0, $zero, 1",
        "addi.d $t0, $zero, 41",
        "bne $fp, $t0, 2f",
        "addi.d $t0, $zero, 42",
        "bne $s0, $t0, 2f",
        "addi.d $t0, $zero, 43",
        "bne $s4, $t0, 2f",
        "addi.d $t0, $zero, 44",
        "bne $s8, $t0, 2f",
        "move $a0, $zero",
        "2:",
        "ld.d $ra, $sp, 0",
        "ld.d $fp, $sp, 8",
        "ld.d $s0, $sp, 16",
        "ld.d $s4, $sp, 24",
        "ld.d $s8, $sp, 32",
        "addi.d $sp, $sp, 48",
        "jirl $zero, $ra, 0",
        switch = sym super::switch_context,
    );
}

#[unsafe(naked)]
#[unsafe(link_section = ".text.test.user")]
pub extern "C" fn user_code() -> ! {
    naked_asm!(
        ".balign 4096",
        "move $s0, $a0",
        "move $s1, $zero",
        ".irp index,0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31",
        "movfr2gr.d $t0, $f\\index",
        "bnez $t0, 9f",
        ".endr",
        "movfcsr2gr $t0, $fcsr0",
        "bnez $t0, 9f",
        ".irp index,0,1,2,3,4,5,6,7",
        "movcf2gr $t0, $fcc\\index",
        "bnez $t0, 9f",
        ".endr",
        ".irp index,0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31",
        "movgr2fr.d $f\\index, $s0",
        ".endr",
        "slli.d $t0, $s0, 8",
        "movgr2fcsr $fcsr0, $t0",
        "andi $t0, $s0, 1",
        ".irp index,0,1,2,3,4,5,6,7",
        "movgr2cf $fcc\\index, $t0",
        ".endr",
        "st.d $s0, $sp, -8",
        "ll.d $t3, $sp, -8",
        "move $t3, $s0",
        "sc.d $t3, $sp, -8",
        "beqz $t3, 9f",
        "2:",
        "move $a0, $s0",
        "move $a1, $s1",
        "move $a2, $tp",
        "move $a3, $sp",
        "addi.d $a7, $zero, 1",
        "ll.d $t3, $sp, -8",
        "syscall 0",
        "move $t3, $s0",
        "sc.d $t3, $sp, -8",
        // SC 成功返回 1，跨 Trap 的预约必须失效。
        "bnez $t3, 9f",
        "bnez $a0, 9f",
        "bne $a1, $s0, 9f",
        "bne $a2, $s1, 9f",
        "slli.d $t0, $s0, 12",
        "bne $tp, $t0, 9f",
        "ld.d $t0, $sp, -8",
        "bne $t0, $s0, 9f",
        ".irp index,0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31",
        "movfr2gr.d $t0, $f\\index",
        "bne $t0, $s0, 9f",
        ".endr",
        "movfcsr2gr $t0, $fcsr0",
        "slli.d $t1, $s0, 8",
        "bne $t0, $t1, 9f",
        "andi $t1, $s0, 1",
        ".irp index,0,1,2,3,4,5,6,7",
        "movcf2gr $t0, $fcc\\index",
        "bne $t0, $t1, 9f",
        ".endr",
        "addi.d $s1, $s1, 1",
        "addi.d $t0, $zero, {rounds}",
        "bltu $s1, $t0, 2b",
        "move $a0, $s0",
        "addi.d $a7, $zero, 2",
        "syscall 0",
        "9:",
        "addi.d $a7, $zero, 99",
        "syscall 0",
        "b .",
        rounds = const super::ROUNDS,
    );
}

#[unsafe(naked)]
#[unsafe(link_section = ".text.test.task_user")]
pub extern "C" fn task_user_code() -> ! {
    naked_asm!(
        ".balign 4096",
        "move $s0, $a0",
        "ld.d $s2, $tp, 0",
        "move $s3, $tp",
        "move $s4, $sp",
        "li.d $s8, 0x5a5a",
        "movgr2fr.d $f0, $s0",
        "movgr2fr.d $f31, $s0",
        "li.d $t0, 256",
        "movgr2fcsr $fcsr0, $t0",
        "li.d $t0, 1",
        "movgr2cf $fcc0, $t0",
        "st.d $s0, $sp, -8",
        "li.d $t0, -2",
        "beq $s2, $t0, 9f",
        "li.d $t0, -3",
        "beq $s2, $t0, 5f",
        "2:",
        "bnez $s2, 3f",
        "st.d $s0, $sp, -16",
        "3:",
        "move $a0, $s0",
        "move $a1, $s2",
        "syscall 0",
        "bnez $a0, 9f",
        "bne $a1, $s0, 9f",
        "bne $a2, $s2, 9f",
        "4:",
        "bne $tp, $s3, 9f",
        "bne $sp, $s4, 9f",
        "li.d $t0, 0x5a5a",
        "bne $s8, $t0, 9f",
        "ld.d $t0, $sp, -8",
        "bne $t0, $s0, 9f",
        "movfr2gr.d $t0, $f0",
        "bne $t0, $s0, 9f",
        "movfr2gr.d $t0, $f31",
        "bne $t0, $s0, 9f",
        "movfcsr2gr $t0, $fcsr0",
        "li.d $t1, 256",
        "bne $t0, $t1, 9f",
        "movcf2gr $t0, $fcc0",
        "li.d $t1, 1",
        "bne $t0, $t1, 9f",
        "li.d $t0, -3",
        "beq $s2, $t0, 7f",
        "beqz $s2, 9f",
        "li.d $t0, -1",
        "beq $s2, $t0, 2b",
        "addi.d $s2, $s2, -1",
        "b 2b",
        "5:",
        "li.d $t2, 0x6161",
        "li.d $a3, 0x7373",
        "li.d $a6, 0x8989",
        "st.d $s0, $tp, 16",
        "6:",
        "ld.d $t0, $tp, 8",
        "beqz $t0, 6b",
        "li.d $t0, 0x6161",
        "bne $t2, $t0, 9f",
        "li.d $t0, 0x7373",
        "bne $a3, $t0, 9f",
        "li.d $t0, 0x8989",
        "bne $a6, $t0, 9f",
        "b 4b",
        "7:",
        "st.d $s0, $sp, -16",
        "move $a0, $s0",
        "move $a1, $zero",
        "syscall 0",
        "9:",
        ".word 0",
        "b .",
    );
}
