// SPDX-License-Identifier: MPL-2.0

//! LoongArch 本地中断与等待指令。

use core::arch::asm;

use kernel::hardware::cpu::{Cpu, InterruptState};

// REFERENCE: https://loongson.github.io/LoongArch-Documentation/LoongArch-Vol1-EN.html
const IE: usize = 1 << 2;

pub(super) struct LoongArchCpu;

crate::declare_hardware!(
    pub(super) static CPU: LoongArchCpu = LoongArchCpu => kernel::hardware::cpu::bind
);

impl Cpu for LoongArchCpu {
    fn interrupt_state(&self) -> InterruptState {
        let status: usize;
        // SAFETY: 执行环境须允许读取 CRMD。
        unsafe { asm!("csrrd {}, 0x0", out(reg) status, options(nomem, nostack)) };
        if status & IE == 0 {
            InterruptState::Disabled
        } else {
            InterruptState::Enabled
        }
    }

    fn disable_interrupts(&self) -> InterruptState {
        let mut status = 0usize;
        // SAFETY: 执行环境须允许修改 CRMD.IE。
        unsafe {
            asm!(
                "csrxchg {status}, {mask}, 0x0",
                status = inout(reg) status,
                mask = in(reg) IE,
                options(nostack),
            );
        }
        if status & IE == 0 {
            InterruptState::Disabled
        } else {
            InterruptState::Enabled
        }
    }

    fn restore_interrupts(&self, state: InterruptState) {
        let value = if state == InterruptState::Enabled {
            IE
        } else {
            0
        };
        // SAFETY: 执行环境须允许修改 CRMD.IE，开中断时异常入口及其栈须已就绪。
        unsafe {
            asm!(
                "csrxchg {value}, {mask}, 0x0",
                value = inout(reg) value => _,
                mask = in(reg) IE,
                options(nostack),
            );
        }
    }

    fn idle_until_interrupt(&self) {
        // IDLE 在 IE 关闭时仍响应已使能的中断源，醒来后开放一次中断处理窗口。
        // SAFETY: 本地中断须关闭，异常入口须可用，执行环境须允许执行 IDLE。
        unsafe {
            asm!(
                "idle 0",
                "csrxchg {value}, {mask}, 0x0",
                "csrxchg $zero, {mask}, 0x0",
                value = inout(reg) IE => _,
                mask = in(reg) IE,
                options(nostack),
            );
        }
    }

    fn halt(&self) -> ! {
        self.disable_interrupts();
        loop {
            // SAFETY: 执行环境须允许在关中断状态执行 IDLE。
            unsafe { asm!("idle 0", options(nomem, nostack)) };
        }
    }
}
