// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! RISC-V local interrupt and wait instructions.

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
        // SAFETY: The execution environment must allow reading sstatus.
        unsafe { asm!("csrr {}, sstatus", out(reg) status, options(nomem, nostack)) };
        if status & SIE == 0 {
            InterruptState::Disabled
        } else {
            InterruptState::Enabled
        }
    }

    fn disable_interrupts(&self) -> InterruptState {
        let status: usize;
        // SAFETY: The execution environment must allow modifying sstatus.SIE.
        unsafe { asm!("csrrci {}, sstatus, 2", out(reg) status, options(nostack)) };
        if status & SIE == 0 {
            InterruptState::Disabled
        } else {
            InterruptState::Enabled
        }
    }

    fn restore_interrupts(&self, state: InterruptState) {
        // SAFETY: The execution environment must allow modifying sstatus.SIE, and the exception entry and its stack must be ready when interrupts are enabled.
        unsafe {
            match state {
                InterruptState::Disabled => asm!("csrci sstatus, 2", options(nostack)),
                InterruptState::Enabled => asm!("csrsi sstatus, 2", options(nostack)),
            }
        }
    }

    fn idle_until_interrupt(&self) {
        // WFI still responds to enabled interrupt sources while SIE is clear, opening one interrupt handling window after waking.
        // SAFETY: Local interrupts must be disabled, the exception entry must be available, and the execution environment must allow executing WFI.
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
            // SAFETY: The execution environment must allow executing WFI with interrupts disabled.
            unsafe { asm!("wfi", options(nomem, nostack)) };
        }
    }
}
