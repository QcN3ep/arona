// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! LoongArch local interrupts and wait instructions.

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
        // SAFETY: The execution environment must permit reading CRMD.
        unsafe { asm!("csrrd {}, 0x0", out(reg) status, options(nomem, nostack)) };
        if status & IE == 0 {
            InterruptState::Disabled
        } else {
            InterruptState::Enabled
        }
    }

    fn disable_interrupts(&self) -> InterruptState {
        let mut status = 0usize;
        // SAFETY: The execution environment must permit modifying CRMD.IE.
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
        // SAFETY: The execution environment must permit modifying CRMD.IE; when enabling interrupts, the exception entry and its stack must already be ready.
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
        // IDLE still responds to enabled interrupt sources while IE is disabled, and opens an interrupt-handling window once it wakes.
        // SAFETY: Local interrupts must be disabled, the exception entry must be available, and the execution environment must permit executing IDLE.
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
            // SAFETY: The execution environment must permit executing IDLE with interrupts disabled.
            unsafe { asm!("idle 0", options(nomem, nostack)) };
        }
    }
}
