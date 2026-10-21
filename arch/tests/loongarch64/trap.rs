// SPDX-License-Identifier: MPL-2.0

use core::arch::{asm, naked_asm};

use kernel::memory::Frame;

pub fn floating_enabled() -> bool {
    let extensions: usize;
    // SAFETY: 执行环境须允许读取 EUEN。
    unsafe { asm!("csrrd {}, 0x2", out(reg) extensions, options(nomem, nostack)) };
    extensions & 1 != 0
}

pub fn floating(enabled: bool) {
    // SAFETY: 测试须独占 EUEN 的浮点扩展配置。
    unsafe {
        asm!(
            "csrxchg {value}, {mask}, 0x2",
            value = inout(reg) usize::from(enabled) => _,
            mask = in(reg) 1usize,
            options(nostack),
        )
    };
}

pub fn acknowledge() {
    // SAFETY: 测试须独占本地软件中断。
    unsafe { asm!("csrxchg $zero, {mask}, 0x5", mask = in(reg) 1usize, options(nostack)) };
}

pub fn clobber_floating() {
    // SAFETY: 浮点扩展须已启用，现场须已由入口保存。
    unsafe {
        asm!(
            "movgr2fr.d $f8, $zero",
            "movgr2fr.d $f0, $zero",
            "movgr2fcsr $fcsr0, $zero",
            "movgr2cf $fcc0, $zero",
            "movgr2cf $fcc7, $zero",
            out("$f8") _,
            out("$f0") _,
            options(nostack),
        )
    };
}

#[unsafe(naked)]
pub extern "C" fn round_trip() -> usize {
    naked_asm!(
        "addi.d $sp, $sp, -48",
        "st.d $ra, $sp, 0",
        "st.d $tp, $sp, 8",
        "st.d $s0, $sp, 16",
        "csrrd $t2, 0x2",
        "andi $t2, $t2, 1",
        "beqz $t2, 2f",
        "movfcsr2gr $t2, $fcsr0",
        "st.d $t2, $sp, 32",
        "movcf2gr $t2, $fcc0",
        "st.b $t2, $sp, 24",
        "movcf2gr $t2, $fcc7",
        "st.b $t2, $sp, 25",
        "addi.d $t2, $zero, 1",
        "movgr2cf $fcc0, $t2",
        "movgr2cf $fcc7, $t2",
        "addi.d $t2, $zero, 73",
        "movgr2fr.d $f8, $t2",
        "addi.d $t2, $zero, 74",
        "movgr2fr.d $f0, $t2",
        "addi.d $t2, $zero, 0x100",
        "movgr2fcsr $fcsr0, $t2",
        "2:",
        "addi.d $t2, $zero, 1",
        "csrwr $t2, 0x4",
        "addi.d $t2, $zero, 1",
        "csrwr $t2, 0x5",
        "addi.d $t0, $zero, 11",
        "addi.d $t1, $zero, 12",
        "addi.d $t3, $zero, 13",
        "addi.d $a0, $zero, 21",
        "addi.d $a1, $zero, 22",
        "addi.d $s0, $zero, 31",
        "addi.d $tp, $zero, 41",
        "addi.d $t2, $zero, 4",
        "csrxchg $t2, $t2, 0x0",
        "addi.d $t2, $zero, 4",
        "csrxchg $zero, $t2, 0x0",
        "addi.d $t2, $zero, 11",
        "bne $t0, $t2, 8f",
        "addi.d $t2, $zero, 12",
        "bne $t1, $t2, 8f",
        "addi.d $t2, $zero, 13",
        "bne $t3, $t2, 8f",
        "addi.d $t2, $zero, 21",
        "bne $a0, $t2, 8f",
        "addi.d $t2, $zero, 22",
        "bne $a1, $t2, 8f",
        "addi.d $t2, $zero, 31",
        "bne $s0, $t2, 8f",
        "addi.d $t2, $zero, 41",
        "bne $tp, $t2, 8f",
        "csrrd $t2, 0x2",
        "andi $t2, $t2, 1",
        "beqz $t2, 3f",
        "movfr2gr.d $t2, $f8",
        "addi.d $t4, $zero, 73",
        "bne $t2, $t4, 8f",
        "movfr2gr.d $t2, $f0",
        "addi.d $t4, $zero, 74",
        "bne $t2, $t4, 8f",
        "movfcsr2gr $t2, $fcsr0",
        "addi.d $t4, $zero, 0x100",
        "bne $t2, $t4, 8f",
        "movcf2gr $t2, $fcc0",
        "beqz $t2, 8f",
        "movcf2gr $t2, $fcc7",
        "beqz $t2, 8f",
        "3:",
        "move $a0, $zero",
        "b 9f",
        "8:",
        "addi.d $a0, $zero, 1",
        "9:",
        "csrrd $t2, 0x2",
        "andi $t2, $t2, 1",
        "beqz $t2, 4f",
        "ld.d $t2, $sp, 32",
        "movgr2fcsr $fcsr0, $t2",
        "ld.bu $t2, $sp, 24",
        "movgr2cf $fcc0, $t2",
        "ld.bu $t2, $sp, 25",
        "movgr2cf $fcc7, $t2",
        "4:",
        "ld.d $ra, $sp, 0",
        "ld.d $tp, $sp, 8",
        "ld.d $s0, $sp, 16",
        "addi.d $sp, $sp, 48",
        "jirl $zero, $ra, 0",
    );
}

#[unsafe(naked)]
pub extern "C" fn fault() -> ! {
    naked_asm!(".word 0", "b .");
}

#[unsafe(naked)]
pub extern "C" fn bounds_fault() -> ! {
    naked_asm!("asrtgt.d $zero, $zero", "b .");
}

#[unsafe(naked)]
pub extern "C" fn arithmetic_fault() -> ! {
    naked_asm!(
        "addi.d $t0, $zero, 1",
        "csrxchg $t0, $t0, 0x2",
        "addi.d $t0, $zero, 16",
        "movgr2fcsr $fcsr0, $t0",
        "movgr2fr.d $f0, $zero",
        "fdiv.d $f0, $f0, $f0",
        "b .",
    );
}

#[unsafe(naked)]
pub extern "C" fn bad_stack() -> ! {
    naked_asm!(
        "addi.d $t0, $zero, 1",
        "csrwr $t0, 0x4",
        "addi.d $t0, $zero, 1",
        "csrwr $t0, 0x5",
        "move $sp, $zero",
        "addi.d $t0, $zero, 4",
        "csrxchg $t0, $t0, 0x0",
        "b .",
    );
}

pub fn user_root(root: Frame) {
    // SAFETY: root 须包含有效的用户测试映射，当前映射须继续覆盖内核。
    unsafe { asm!("csrwr {}, 0x19", inout(reg) root.address() => _, options(nostack)) };
}

#[unsafe(naked)]
pub extern "C" fn enter_user() -> ! {
    naked_asm!(
        "li.d $t0, 0x10000",
        "csrwr $t0, 0x6",
        "addi.d $t0, $zero, 3",
        "csrwr $t0, 0x1",
        "li.d $sp, 0x21000",
        "ertn",
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
        "addi.d $a0, $zero, 1",
        "addi.d $a1, $zero, 2",
        "addi.d $a2, $zero, 3",
        "addi.d $a3, $zero, 4",
        "addi.d $a4, $zero, 5",
        "addi.d $a5, $zero, 6",
        "addi.d $a7, $zero, 17",
        "syscall 0",
        "addi.d $t0, $zero, 42",
        "bne $a0, $t0, 2f",
        "addi.d $t0, $zero, 73",
        "bne $a1, $t0, 2f",
        "addi.d $t0, $zero, 19",
        "bne $a2, $t0, 2f",
        "li.d $t0, 0x21000",
        "bne $sp, $t0, 2f",
        "li.d $t0, 0x30000",
        "ld.d $t1, $t0, 0",
        "li.d $t0, 0x31000",
        "st.d $t1, $t0, 0",
        "li.d $t0, 0x32000",
        "jirl $zero, $t0, 0",
        "2:",
        "addi.d $a7, $zero, 99",
        "syscall 0",
        "b .",
    );
}

#[unsafe(naked)]
#[unsafe(link_section = ".text.test.user.finish")]
pub extern "C" fn user_finish() -> ! {
    naked_asm!(".balign 4096", "addi.d $a7, $zero, 18", "syscall 0", "b .");
}
