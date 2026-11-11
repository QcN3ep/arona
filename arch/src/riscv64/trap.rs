// SPDX-License-Identifier: MPL-2.0

use core::{
    arch::{asm, naked_asm},
    mem::{MaybeUninit, offset_of, size_of},
};

use kernel::{
    hardware::{
        cpu::{Cpu, InterruptState},
        trap::{Handler, TrapControl, TrapError},
    },
    memory::VirtRange,
    trap::{ExceptionClass, FaultAccess, Trap, TrapEvent, TrapOrigin, TrapOutcome},
};

use super::{boot::EARLY_CONSOLE, cpu::CPU, execution};
use crate::trap::{EMERGENCY_STACK_SIZE, EmergencyStack, EntryReason, EntryState, STACK_HEADROOM};

crate::declare_hardware!(static TRAP: RiscvTrap = RiscvTrap => kernel::hardware::trap::bind);

pub(super) static mut STATE: EntryState = EntryState::new();
static mut EMERGENCY_STACK: EmergencyStack = EmergencyStack([0; EMERGENCY_STACK_SIZE]);

#[repr(C, align(16))]
pub(super) struct TrapFrame {
    pub(super) registers: [usize; 32],
    pub(super) pc: usize,
    pub(super) status: usize,
    pub(super) cause: usize,
    pub(super) address: usize,
    pub(super) fcsr: usize,
    pub(super) reason: EntryReason,
    pub(super) floating: MaybeUninit<[u64; 32]>,
}

struct RiscvTrap;

impl TrapControl for RiscvTrap {
    fn install(&self, stack: VirtRange, handler: Handler) -> Result<(), TrapError> {
        if CPU.interrupt_state() != InterruptState::Disabled {
            return Err(TrapError::InterruptsEnabled);
        }
        // SAFETY: The current processor must exclusively own the entry state and install it with interrupts disabled.
        let state = unsafe { &mut *core::ptr::addr_of_mut!(STATE) };
        if state.handler.is_some() {
            return Err(TrapError::AlreadyInstalled);
        }
        state.set_stack(stack, size_of::<TrapFrame>())?;
        state.emergency_top = core::ptr::addr_of!(EMERGENCY_STACK) as usize + EMERGENCY_STACK_SIZE;
        state.handler = Some(handler);
        // SAFETY: The entry, its static state, and the stack must remain mapped, and the current processor must exclusively own sscratch.
        unsafe {
            asm!(
                "mv {tp}, tp",
                "mv {gp}, gp",
                tp = out(reg) state.kernel_tp,
                gp = out(reg) state.kernel_gp,
                options(nomem, nostack),
            );
            asm!(
                "csrw sscratch, {state}",
                "csrw stvec, {entry}",
                state = in(reg) core::ptr::addr_of!(STATE),
                entry = in(reg) entry as *const () as usize,
                options(nostack),
            );
        }
        Ok(())
    }

    fn set_stack(&self, stack: VirtRange) -> Result<(), TrapError> {
        if CPU.interrupt_state() != InterruptState::Disabled {
            return Err(TrapError::InterruptsEnabled);
        }
        // SAFETY: The current processor must exclusively own the entry state, and local interrupts must be disabled.
        let state = unsafe { &mut *core::ptr::addr_of_mut!(STATE) };
        if state.handler.is_none() {
            return Err(TrapError::NotInstalled);
        }
        state.set_stack(stack, size_of::<TrapFrame>())
    }
}

// REFERENCE: https://docs.riscv.org/reference/isa/priv/supervisor.html

/// # Safety
///
/// sscratch must point to the current processor's resident entry state, and the entry code and emergency stack must always be accessible.
#[unsafe(naked)]
extern "C" fn entry() -> ! {
    naked_asm!(
        ".balign 4",
        "csrrw t0, sscratch, t0",
        "sd t1, {scratch}+8(t0)",
        "sd t2, {scratch}+16(t0)",
        "sd t3, {scratch}+24(t0)",
        "csrr t1, sscratch",
        "sd t1, {scratch}(t0)",
        "csrw sscratch, t0",
        "ld t1, {active}(t0)",
        "li t2, 2",
        "beq t1, t2, 9f",
        "bnez t1, 6f",
        "csrr t1, sstatus",
        "andi t1, t1, 256",
        "beqz t1, 2f",
        "csrr t1, scause",
        "bgez t1, 4f",
        "mv t1, sp",
        "j 3f",
        "2:",
        "ld t1, {end}(t0)",
        "3:",
        "andi t2, t1, 15",
        "bnez t2, 5f",
        "ld t2, {end}(t0)",
        "bgtu t1, t2, 5f",
        "ld t2, {start}(t0)",
        "bltu t1, t2, 5f",
        "sub t2, t1, t2",
        "li t3, {minimum}",
        "bltu t2, t3, 5f",
        "li t2, 1",
        "sd t2, {active}(t0)",
        "li t3, {normal}",
        "j 8f",
        "4:",
        "li t3, {kernel_exception}",
        "j 7f",
        "5:",
        "li t3, {invalid_stack}",
        "j 7f",
        "6:",
        "li t3, {reentrant}",
        "7:",
        "ld t1, {emergency}(t0)",
        "li t2, 2",
        "sd t2, {active}(t0)",
        "8:",
        "addi t1, t1, -{frame_size}",
        "sd sp, 16(t1)",
        "mv sp, t1",
        "sd t3, {reason}(sp)",
        "tail {save_frame}",
        "9:",
        "tail {double_fault}",
        start = const offset_of!(EntryState, stack_start),
        end = const offset_of!(EntryState, stack_end),
        active = const offset_of!(EntryState, active),
        emergency = const offset_of!(EntryState, emergency_top),
        scratch = const offset_of!(EntryState, scratch),
        frame_size = const size_of::<TrapFrame>(),
        minimum = const size_of::<TrapFrame>() + STACK_HEADROOM,
        reason = const offset_of!(TrapFrame, reason),
        normal = const EntryReason::Normal as usize,
        kernel_exception = const EntryReason::KernelException as usize,
        invalid_stack = const EntryReason::InvalidStack as usize,
        reentrant = const EntryReason::Reentrant as usize,
        save_frame = sym save_frame,
        double_fault = sym double_fault,
    );
}

#[unsafe(naked)]
extern "C" fn save_frame() -> ! {
    naked_asm!(
        "sd zero, 0(sp)",
        "sd ra, 8(sp)",
        "sd gp, 24(sp)",
        "sd tp, 32(sp)",
        "sd s0, 64(sp)",
        "sd s1, 72(sp)",
        "sd a0, 80(sp)",
        "sd a1, 88(sp)",
        "sd a2, 96(sp)",
        "sd a3, 104(sp)",
        "sd a4, 112(sp)",
        "sd a5, 120(sp)",
        "sd a6, 128(sp)",
        "sd a7, 136(sp)",
        "sd s2, 144(sp)",
        "sd s3, 152(sp)",
        "sd s4, 160(sp)",
        "sd s5, 168(sp)",
        "sd s6, 176(sp)",
        "sd s7, 184(sp)",
        "sd s8, 192(sp)",
        "sd s9, 200(sp)",
        "sd s10, 208(sp)",
        "sd s11, 216(sp)",
        "sd t4, 232(sp)",
        "sd t5, 240(sp)",
        "sd t6, 248(sp)",
        "ld t1, {scratch}(t0)",
        "sd t1, 40(sp)",
        "ld t1, {scratch}+8(t0)",
        "sd t1, 48(sp)",
        "ld t1, {scratch}+16(t0)",
        "sd t1, 56(sp)",
        "ld t1, {scratch}+24(t0)",
        "sd t1, 224(sp)",
        "csrr t1, sepc",
        "sd t1, {pc}(sp)",
        "csrr t1, sstatus",
        "sd t1, {status}(sp)",
        "csrr t1, scause",
        "sd t1, {cause}(sp)",
        "csrr t1, stval",
        "sd t1, {address}(sp)",
        "sd zero, {fcsr}(sp)",
        "ld tp, {tp}(t0)",
        "ld gp, {gp}(t0)",
        "mv a0, sp",
        "call {save_floating}",
        "mv a0, sp",
        "call {capture}",
        "call {handle}",
        "csrr t0, sscratch",
        "sd zero, {active}(t0)",
        "mv sp, a0",
        "tail {restore}",
        active = const offset_of!(EntryState, active),
        scratch = const offset_of!(EntryState, scratch),
        tp = const offset_of!(EntryState, kernel_tp),
        gp = const offset_of!(EntryState, kernel_gp),
        pc = const offset_of!(TrapFrame, pc),
        status = const offset_of!(TrapFrame, status),
        cause = const offset_of!(TrapFrame, cause),
        address = const offset_of!(TrapFrame, address),
        fcsr = const offset_of!(TrapFrame, fcsr),
        capture = sym execution::capture,
        save_floating = sym execution::save_floating,
        handle = sym handle,
        restore = sym execution::restore_frame,
    );
}

#[unsafe(naked)]
extern "C" fn double_fault() -> ! {
    naked_asm!("csrci sstatus, 2", "2:", "wfi", "j 2b");
}

extern "C" fn handle(frame: *mut TrapFrame) -> *const TrapFrame {
    // SAFETY: frame must be the valid frame of the current entry.
    let (trap, reason) = unsafe { ((*frame).decode(), (*frame).reason) };
    if reason == EntryReason::Normal {
        // SAFETY: STATE must already be installed, and local interrupts must be disabled during access.
        let handler = unsafe { (*core::ptr::addr_of!(STATE)).handler }.unwrap();
        match handler(trap) {
            TrapOutcome::Resume => return frame,
            TrapOutcome::Return { status, values } => {
                if let TrapEvent::Syscall { successor, .. } = trap.event {
                    // SAFETY: After the handler returns, the frame must still be exclusively owned by the current entry.
                    let frame = unsafe { &mut *frame };
                    frame.registers[10..13].copy_from_slice(&[status, values[0], values[1]]);
                    frame.pc = successor;
                    return frame;
                }
            }
            TrapOutcome::Stop => {}
        }
    }
    // SAFETY: Before stopping, the frame must still be exclusively owned by the current entry.
    let frame = unsafe { &*frame };
    EARLY_CONSOLE.emergency_write(format_args!(
        "\nFATAL TRAP: {:?} at {:#x}: {:?}\nsp={:#x} cause={:#x} entry={:?}\n",
        trap.origin, trap.pc, trap.event, frame.registers[2], frame.cause, frame.reason,
    ));
    CPU.halt()
}

impl TrapFrame {
    fn decode(&self) -> Trap {
        let origin = if self.status & (1 << 8) == 0 {
            TrapOrigin::User
        } else {
            TrapOrigin::Kernel
        };
        let event = if self.cause >> (usize::BITS - 1) != 0 {
            match self.cause & isize::MAX as usize {
                1 => TrapEvent::SoftwareInterrupt,
                5 => TrapEvent::TimerInterrupt,
                9 => TrapEvent::ExternalInterrupt,
                _ => TrapEvent::OtherInterrupt,
            }
        } else {
            self.exception(origin)
        };
        Trap {
            origin,
            pc: self.pc,
            event,
        }
    }

    fn exception(&self, origin: TrapOrigin) -> TrapEvent {
        match self.cause {
            8 if origin == TrapOrigin::User => TrapEvent::Syscall {
                handle: self.registers[17],
                arguments: self.registers[10..16].try_into().unwrap(),
                successor: self.pc + 4,
            },
            12 | 13 | 15 => TrapEvent::PageFault {
                address: self.address,
                access: match self.cause {
                    12 => FaultAccess::Execute,
                    13 => FaultAccess::Read,
                    _ => FaultAccess::Write,
                },
            },
            2 => TrapEvent::IllegalInstruction {
                instruction: u32::try_from(self.address).ok().filter(|word| *word != 0),
            },
            _ => TrapEvent::OtherException {
                class: match self.cause {
                    0 | 4 | 6 => ExceptionClass::MisalignedAddress,
                    1 | 5 | 7 => ExceptionClass::AccessFault,
                    3 => ExceptionClass::Breakpoint,
                    _ => ExceptionClass::Unknown,
                },
                address: match self.cause {
                    0 | 1 | 4..=7 => Some(self.address),
                    _ => None,
                },
            },
        }
    }
}
