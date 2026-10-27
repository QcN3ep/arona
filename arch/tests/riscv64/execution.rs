// SPDX-License-Identifier: MPL-2.0

use core::arch::{asm, naked_asm};
use kernel::hardware::execution::Context;

pub fn frequency(dtb: usize) -> Option<u64> {
    let pointer = (0xffff_ffc0_0040_0000 + (dtb & 0x1f_ffff)) as *const u32;
    // SAFETY: 固件 DTB 须完整映射到启动固件窗口且解析期间保持不变。
    let tree = unsafe { fdt::Fdt::from_ptr_fallible(pointer) }.unwrap();
    tree.root()
        .unwrap()
        .cpus()
        .unwrap()
        .common_timebase_frequency()
        .unwrap()
}

pub fn set_floating_enabled(enabled: bool) {
    // SAFETY: 当前处理器须允许修改 sstatus，修改期间不得执行浮点指令。
    unsafe {
        if enabled {
            asm!("csrs sstatus, {}", in(reg) 2usize << 13, options(nomem, nostack));
        } else {
            asm!("csrc sstatus, {}", in(reg) 3usize << 13, options(nomem, nostack));
        }
    }
}

pub fn floating_enabled() -> bool {
    let status: usize;
    // SAFETY: 执行环境须允许读取 sstatus。
    unsafe {
        asm!("csrr {}, sstatus", out(reg) status, options(nomem, nostack));
    }
    status & 0x6000 != 0
}

#[unsafe(naked)]
pub extern "C" fn switch(_current: &Context, _next: &Context) -> usize {
    naked_asm!(
        "addi sp, sp, -48",
        "sd ra, 0(sp)",
        "sd s0, 8(sp)",
        "sd s1, 16(sp)",
        "sd s5, 24(sp)",
        "sd s11, 32(sp)",
        "li s0, 41",
        "li s1, 42",
        "li s5, 43",
        "li s11, 44",
        "call {switch}",
        "li a0, 1",
        "li t0, 41",
        "bne s0, t0, 2f",
        "li t0, 42",
        "bne s1, t0, 2f",
        "li t0, 43",
        "bne s5, t0, 2f",
        "li t0, 44",
        "bne s11, t0, 2f",
        "li a0, 0",
        "2:",
        "ld ra, 0(sp)",
        "ld s0, 8(sp)",
        "ld s1, 16(sp)",
        "ld s5, 24(sp)",
        "ld s11, 32(sp)",
        "addi sp, sp, 48",
        "ret",
        switch = sym super::switch_context,
    );
}

#[unsafe(naked)]
#[unsafe(link_section = ".text.test.user")]
pub extern "C" fn user_code() -> ! {
    naked_asm!(
        ".balign 4096",
        ".option push",
        ".option arch, +a, +f, +d",
        ".option norvc",
        "mv s0, a0",
        "li s1, 0",
        ".irp index,0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31",
        "fmv.x.d t0, f\\index",
        "bnez t0, 9f",
        ".endr",
        "frcsr t0",
        "bnez t0, 9f",
        ".irp index,0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31",
        "fmv.d.x f\\index, s0",
        ".endr",
        "slli t0, s0, 5",
        "fscsr t0",
        "sd s0, -8(sp)",
        "addi t2, sp, -8",
        "lr.d t3, (t2)",
        "sc.d t3, s0, (t2)",
        "bnez t3, 9f",
        "2:",
        "mv a0, s0",
        "mv a1, s1",
        "mv a2, tp",
        "mv a3, sp",
        "li a7, 1",
        "addi t2, sp, -8",
        "lr.d t3, (t2)",
        "ecall",
        "sc.d t3, s0, (t2)",
        // SC 成功返回 0，跨 Trap 的预约必须失效。
        "beqz t3, 9f",
        "bnez a0, 9f",
        "bne a1, s0, 9f",
        "bne a2, s1, 9f",
        "slli t0, s0, 12",
        "bne tp, t0, 9f",
        "ld t0, -8(sp)",
        "bne t0, s0, 9f",
        ".irp index,0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31",
        "fmv.x.d t0, f\\index",
        "bne t0, s0, 9f",
        ".endr",
        "frcsr t0",
        "slli t1, s0, 5",
        "bne t0, t1, 9f",
        "addi s1, s1, 1",
        "li t0, {rounds}",
        "bltu s1, t0, 2b",
        "mv a0, s0",
        "li a7, 2",
        "ecall",
        "9:",
        "li a7, 99",
        "ecall",
        "j .",
        ".option pop",
        rounds = const super::ROUNDS,
    );
}

#[unsafe(naked)]
#[unsafe(link_section = ".text.test.task_user")]
pub extern "C" fn task_user_code() -> ! {
    naked_asm!(
        ".balign 4096",
        ".option push",
        ".option arch, +f, +d",
        ".option norvc",
        "mv s0, a0",
        "ld s2, 0(tp)",
        "mv s3, tp",
        "mv s4, sp",
        "li s11, 0x5a5a",
        "fmv.d.x f0, s0",
        "fmv.d.x f31, s0",
        "li t0, 64",
        "fscsr t0",
        "sd s0, -8(sp)",
        "li t0, -2",
        "beq s2, t0, 9f",
        "li t0, -3",
        "beq s2, t0, 5f",
        "2:",
        "bnez s2, 3f",
        "sd s0, -16(sp)",
        "3:",
        "mv a0, s0",
        "mv a1, s2",
        "ecall",
        "bnez a0, 9f",
        "bne a1, s0, 9f",
        "bne a2, s2, 9f",
        "4:",
        "bne tp, s3, 9f",
        "bne sp, s4, 9f",
        "li t0, 0x5a5a",
        "bne s11, t0, 9f",
        "ld t0, -8(sp)",
        "bne t0, s0, 9f",
        "fmv.x.d t0, f0",
        "bne t0, s0, 9f",
        "fmv.x.d t0, f31",
        "bne t0, s0, 9f",
        "frcsr t0",
        "li t1, 64",
        "bne t0, t1, 9f",
        "li t0, -3",
        "beq s2, t0, 7f",
        "beqz s2, 9f",
        "li t0, -1",
        "beq s2, t0, 2b",
        "addi s2, s2, -1",
        "j 2b",
        "5:",
        "li t2, 0x6161",
        "li a3, 0x7373",
        "li a6, 0x8989",
        "sd s0, 16(tp)",
        "6:",
        "ld t0, 8(tp)",
        "beqz t0, 6b",
        "li t0, 0x6161",
        "bne t2, t0, 9f",
        "li t0, 0x7373",
        "bne a3, t0, 9f",
        "li t0, 0x8989",
        "bne a6, t0, 9f",
        "j 4b",
        "7:",
        "sd s0, -16(sp)",
        "mv a0, s0",
        "li a1, 0",
        "ecall",
        "9:",
        ".word 0",
        "j .",
        ".option pop",
    );
}

#[unsafe(naked)]
#[unsafe(link_section = ".text.test.syscall_user")]
pub extern "C" fn syscall_user_code() -> ! {
    naked_asm!(
        ".balign 4096",
        ".option push",
        ".option norvc",
        "mv s0, a0",
        "mv a7, s0",
        "li a0, 0",
        "ecall",
        "bnez a0, 9f",
        "li t0, 1",
        "bne a1, t0, 9f",
        "mv a7, s0",
        "li a0, 2",
        "li a1, 0",
        "ecall",
        "bnez a0, 9f",
        "mv s1, a1",
        "mv a7, s1",
        "li a0, 2",
        "li a1, 0",
        "ecall",
        "li t0, 3",
        "bne a0, t0, 9f",
        "mv a7, s1",
        "li a0, 999",
        "ecall",
        "li t0, 15",
        "bne a0, t0, 9f",
        "mv a7, s1",
        "li a0, 1",
        "ecall",
        "bnez a0, 9f",
        "mv a7, s1",
        "li a0, 0",
        "ecall",
        "li t0, 1",
        "bne a0, t0, 9f",
        "li t0, 73",
        "sd t0, 0(tp)",
        "9:",
        ".word 0",
        ".option pop",
        ".balign 4096",
    );
}
