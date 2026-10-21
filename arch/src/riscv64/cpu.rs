// SPDX-License-Identifier: MPL-2.0

//! RISC-V 本地中断与等待指令。

use core::arch::asm;

use kernel::hardware::cpu::{Cpu, InterruptState};

// REFERENCE: https://docs.riscv.org/reference/isa/priv/supervisor.html
const SIE: usize = 1 << 1;

pub(super) struct RiscvCpu;

crate::declare_hardware!(
    pub(super) static CPU: RiscvCpu = RiscvCpu => kernel::hardware::cpu::bind
);

impl Cpu for RiscvCpu {
    fn interrupt_state(&self) -> InterruptState {
        let status: usize;
        // SAFETY: 执行环境须允许读取 sstatus。
        unsafe { asm!("csrr {}, sstatus", out(reg) status, options(nomem, nostack)) };
        if status & SIE == 0 {
            InterruptState::Disabled
        } else {
            InterruptState::Enabled
        }
    }

    fn disable_interrupts(&self) -> InterruptState {
        let status: usize;
        // SAFETY: 执行环境须允许修改 sstatus.SIE。
        unsafe { asm!("csrrci {}, sstatus, 2", out(reg) status, options(nostack)) };
        if status & SIE == 0 {
            InterruptState::Disabled
        } else {
            InterruptState::Enabled
        }
    }

    fn restore_interrupts(&self, state: InterruptState) {
        // SAFETY: 执行环境须允许修改 sstatus.SIE，开中断时异常入口及其栈须已就绪。
        unsafe {
            match state {
                InterruptState::Disabled => asm!("csrci sstatus, 2", options(nostack)),
                InterruptState::Enabled => asm!("csrsi sstatus, 2", options(nostack)),
            }
        }
    }

    fn idle_until_interrupt(&self) {
        // WFI 在 SIE 关闭时仍响应已使能的中断源，醒来后开放一次中断处理窗口。
        // SAFETY: 本地中断须关闭，异常入口须可用，执行环境须允许执行 WFI。
        unsafe {
            asm!(
                "wfi",
                "csrsi sstatus, 2",
                "csrci sstatus, 2",
                options(nostack)
            );
        }
    }

    fn halt(&self) -> ! {
        self.disable_interrupts();
        loop {
            // SAFETY: 执行环境须允许在关中断状态执行 WFI。
            unsafe { asm!("wfi", options(nomem, nostack)) };
        }
    }
}
