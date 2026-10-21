// SPDX-License-Identifier: MPL-2.0

use core::arch::{asm, naked_asm};

use kernel::memory::Frame;

pub fn floating_enabled() -> bool {
    let status: usize;
    // SAFETY: 执行环境须允许读取 sstatus。
    unsafe { asm!("csrr {}, sstatus", out(reg) status, options(nomem, nostack)) };
    status & 0x6000 != 0
}

pub fn floating(enabled: bool) {
    // SAFETY: 测试须独占 sstatus 的浮点状态配置。
    unsafe {
        asm!(
            "csrc sstatus, {mask}",
            "csrs sstatus, {value}",
            mask = in(reg) 0x6000usize,
            value = in(reg) if enabled { 0x4000usize } else { 0 },
            options(nostack),
        )
    };
}

pub fn acknowledge() {
    // SAFETY: 测试须独占监督态软件中断。
    unsafe { asm!("csrci sip, 2", options(nostack)) };
}

pub fn clobber_floating() {
    // SAFETY: 浮点扩展须已启用，现场须已由入口保存。
    unsafe {
        asm!(
            "fmv.d.x ft0, zero",
            "fmv.d.x fa0, zero",
            "fscsr zero",
            out("ft0") _,
            out("fa0") _,
            options(nostack),
        )
    };
}

#[unsafe(naked)]
pub extern "C" fn round_trip() -> usize {
    naked_asm!(
        ".option push",
        ".option arch, +f, +d",
        "addi sp, sp, -48",
        "sd ra, 0(sp)",
        "sd gp, 8(sp)",
        "sd tp, 16(sp)",
        "sd s0, 24(sp)",
        "csrr t2, sstatus",
        "li t4, 0x6000",
        "and t2, t2, t4",
        "beqz t2, 2f",
        "frcsr t2",
        "sd t2, 32(sp)",
        "li t2, 73",
        "fmv.d.x ft0, t2",
        "li t2, 74",
        "fmv.d.x fa0, t2",
        "li t2, 32",
        "fscsr t2",
        "2:",
        "csrsi sie, 2",
        "csrsi sip, 2",
        "li t0, 11",
        "li t1, 12",
        "li t3, 13",
        "li a0, 21",
        "li a1, 22",
        "li s0, 31",
        "li tp, 41",
        "li gp, 42",
        "csrsi sstatus, 2",
        "csrci sstatus, 2",
        "li t2, 11",
        "bne t0, t2, 8f",
        "li t2, 12",
        "bne t1, t2, 8f",
        "li t2, 13",
        "bne t3, t2, 8f",
        "li t2, 21",
        "bne a0, t2, 8f",
        "li t2, 22",
        "bne a1, t2, 8f",
        "li t2, 31",
        "bne s0, t2, 8f",
        "li t2, 41",
        "bne tp, t2, 8f",
        "li t2, 42",
        "bne gp, t2, 8f",
        "csrr t2, sstatus",
        "li t4, 0x6000",
        "and t2, t2, t4",
        "beqz t2, 3f",
        "fmv.x.d t2, ft0",
        "li t4, 73",
        "bne t2, t4, 8f",
        "fmv.x.d t2, fa0",
        "li t4, 74",
        "bne t2, t4, 8f",
        "frcsr t2",
        "li t4, 32",
        "bne t2, t4, 8f",
        "3:",
        "li a0, 0",
        "j 9f",
        "8:",
        "li a0, 1",
        "9:",
        "csrr t2, sstatus",
        "li t4, 0x6000",
        "and t2, t2, t4",
        "beqz t2, 4f",
        "ld t2, 32(sp)",
        "fscsr t2",
        "4:",
        "ld ra, 0(sp)",
        "ld gp, 8(sp)",
        "ld tp, 16(sp)",
        "ld s0, 24(sp)",
        "addi sp, sp, 48",
        "ret",
        ".option pop",
    );
}

#[unsafe(naked)]
pub extern "C" fn fault() -> ! {
    naked_asm!(".word 0", "j .");
}

#[unsafe(naked)]
pub extern "C" fn bad_stack() -> ! {
    naked_asm!(
        "csrsi sie, 2",
        "csrsi sip, 2",
        "li sp, 0",
        "csrsi sstatus, 2",
        "j ."
    );
}

pub fn user_root(_root: Frame) {}

#[unsafe(naked)]
pub extern "C" fn enter_user() -> ! {
    naked_asm!(
        "li t0, 0x10000",
        "csrw sepc, t0",
        "li t0, 0x100",
        "csrc sstatus, t0",
        "li sp, 0x21000",
        "sret",
    );
}

pub fn syscall_successor() -> usize {
    32
}

#[unsafe(naked)]
#[unsafe(link_section = ".text.test.user")]
pub extern "C" fn user_code() -> ! {
    naked_asm!(
        ".balign 4096",
        ".option push",
        ".option norvc",
        "li a0, 1",
        "li a1, 2",
        "li a2, 3",
        "li a3, 4",
        "li a4, 5",
        "li a5, 6",
        "li a7, 17",
        "ecall",
        "li t0, 42",
        "bne a0, t0, 2f",
        "li t0, 73",
        "bne a1, t0, 2f",
        "li t0, 19",
        "bne a2, t0, 2f",
        "li t0, 0x21000",
        "bne sp, t0, 2f",
        "li t0, 0x30000",
        "ld t1, 0(t0)",
        "li t0, 0x31000",
        "sd t1, 0(t0)",
        "li t0, 0x32000",
        "jr t0",
        "2:",
        "li a7, 99",
        "ecall",
        "j .",
        ".option pop",
    );
}

#[unsafe(naked)]
#[unsafe(link_section = ".text.test.user.finish")]
pub extern "C" fn user_finish() -> ! {
    naked_asm!(".balign 4096", "li a7, 18", "ecall", "j .");
}
