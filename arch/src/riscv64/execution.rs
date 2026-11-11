// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use core::{
    alloc::Layout,
    arch::{asm, naked_asm},
    convert::Infallible,
    mem::{MaybeUninit, offset_of, size_of},
};

use kernel::hardware::{
    cpu::{Cpu, InterruptState},
    execution::{ContextStart, ContextStorage, Execution, ExecutionError, UserEntry, UserState},
};

use super::{
    cpu::CPU,
    trap::{self, TrapFrame},
};
use crate::execution::{
    ContextStatus, EntryContext, SavedContext, activate, capture_frame, pointer,
    validate_user_access, validate_user_entry,
};

const SIE: usize = 1 << 1;
const SPIE: usize = 1 << 5;
const SPP: usize = 1 << 8;
const VS: usize = 3 << 9;
const FS: usize = 3 << 13;
const FS_CLEAN: usize = 2 << 13;
const SUM: usize = 1 << 18;
const MXR: usize = 1 << 19;

crate::declare_hardware!(static EXECUTION: RiscvExecution = RiscvExecution => kernel::hardware::execution::bind);

type Context = SavedContext<TrapFrame, KernelContinuation>;

#[derive(Default)]
#[repr(C)]
struct KernelContinuation {
    ra: usize,
    sp: usize,
    gp: usize,
    tp: usize,
    saved: [usize; 12],
}

struct RiscvExecution;

impl Execution for RiscvExecution {
    fn context_layout(&self) -> Layout {
        Layout::new::<Context>()
    }

    fn user_entry(&self, state: &UserState) -> Result<UserEntry, ExecutionError> {
        if state.registers[0] != 0
            || state.floating_control & !0xff != 0
            || state.floating_control >> 5 > 4
            || state.floating_conditions != [0; 8]
        {
            return Err(ExecutionError::InvalidState);
        }
        validate_user_entry(
            UserEntry {
                pc: state.pc,
                stack_pointer: state.registers[2],
                thread_pointer: state.registers[4],
                argument: state.registers[10],
            },
            2,
        )
    }

    fn user_state(&self, storage: &ContextStorage) -> Result<UserState, ExecutionError> {
        check_environment()?;
        let context = user_context(storage)?;
        // SAFETY: The context must be exclusively held by the current processor with interrupts disabled.
        let frame = unsafe { &(*context).frame };
        Ok(UserState {
            pc: frame.pc,
            registers: frame.registers,
            floating: if frame.status & FS != 0 {
                // SAFETY: When FS is non-zero, the entry must have saved all floating-point registers.
                unsafe { frame.floating.assume_init() }
            } else {
                [0; 32]
            },
            floating_control: frame.fcsr as u32,
            floating_conditions: [0; 8],
        })
    }

    fn set_user_state(
        &self,
        storage: &ContextStorage,
        state: &UserState,
    ) -> Result<(), ExecutionError> {
        check_environment()?;
        self.user_entry(state)?;
        let context = user_context(storage)?;
        // SAFETY: The context must be exclusively held by the current processor with interrupts disabled.
        let frame = unsafe { &mut (*context).frame };
        frame.pc = state.pc;
        frame.registers = state.registers;
        frame.floating = MaybeUninit::new(state.floating);
        frame.fcsr = state.floating_control as usize;
        frame.status = (frame.status & !FS) | FS_CLEAN;
        Ok(())
    }

    fn initialize(
        &self,
        storage: &ContextStorage,
        start: ContextStart,
    ) -> Result<(), ExecutionError> {
        check_environment()?;
        let destination = pointer::<Context>(storage)?;
        // SAFETY: The entry state must be exclusively held by the current processor with interrupts disabled.
        let state = unsafe { &mut *core::ptr::addr_of_mut!(trap::STATE) };
        if matches!(start, ContextStart::Current) && !state.context.is_null() {
            return Err(ExecutionError::InvalidState);
        }
        let entry = EntryContext::initial(start, size_of::<TrapFrame>(), 2)?;
        let (status, gp, tp): (usize, usize, usize);
        // SAFETY: The current processor must allow reading the privileged status and kernel ABI registers.
        unsafe {
            asm!(
                "csrr {status}, sstatus",
                "mv {gp}, gp",
                "mv {tp}, tp",
                status = out(reg) status,
                gp = out(reg) gp,
                tp = out(reg) tp,
                options(nomem, nostack),
            );
        }
        let mut frame = TrapFrame {
            registers: [0; 32],
            pc: 0,
            status: (status & !(FS | VS | SIE | SPIE | SUM | MXR)) | SPP,
            cause: 0,
            address: 0,
            fcsr: 0,
            reason: crate::trap::EntryReason::Normal,
            floating: MaybeUninit::new([0; 32]),
        };
        let mut continuation = KernelContinuation {
            tp,
            gp,
            ..KernelContinuation::default()
        };
        match start {
            ContextStart::Current => {}
            ContextStart::Kernel {
                stack,
                entry,
                argument,
            } => {
                continuation.ra = start_kernel as *const () as usize;
                continuation.sp = stack.end;
                continuation.saved[0] = entry as usize;
                continuation.saved[1] = argument;
            }
            ContextStart::User { stack, entry } => {
                continuation.ra = start_user as *const () as usize;
                continuation.sp = stack.end;
                continuation.saved[0] = destination as usize;
                frame.status = (frame.status & !SPP) | SPIE | FS_CLEAN;
                frame.pc = entry.pc;
                frame.registers[2] = entry.stack_pointer;
                frame.registers[4] = entry.thread_pointer;
                frame.registers[10] = entry.argument;
            }
        }
        // SAFETY: destination must be stable storage exclusively owned by this context and allocated according to the declared layout.
        unsafe {
            destination.write(Context {
                frame,
                continuation,
                entry,
                status: ContextStatus::initial(start),
            });
        }
        if matches!(start, ContextStart::Current) {
            state.context = destination.cast();
        }
        Ok(())
    }

    fn switch(
        &self,
        current: &ContextStorage,
        next: &ContextStorage,
    ) -> Result<(), ExecutionError> {
        check_environment()?;
        let current = pointer::<Context>(current)?;
        let next = pointer::<Context>(next)?;
        // SAFETY: Both contexts must be exclusive and initialized, and the entry state must not be accessed concurrently.
        unsafe {
            activate(
                current,
                next,
                &mut *core::ptr::addr_of_mut!(trap::STATE),
                ContextStatus::Suspended,
            )?;
            switch(core::ptr::addr_of_mut!((*current).continuation), next);
        }
        Ok(())
    }

    fn switch_current(&self, next: &ContextStorage) -> Result<(), ExecutionError> {
        check_environment()?;
        let next = pointer::<Context>(next)?;
        // SAFETY: The current context and target storage must be alive, and the entry state must be exclusively held by the current processor.
        unsafe {
            let state = &mut *core::ptr::addr_of_mut!(trap::STATE);
            let current = state.context.cast::<Context>();
            activate(current, next, state, ContextStatus::Suspended)?;
            switch(core::ptr::addr_of_mut!((*current).continuation), next);
        }
        Ok(())
    }

    fn restore(
        &self,
        current: &ContextStorage,
        next: &ContextStorage,
    ) -> Result<Infallible, ExecutionError> {
        check_environment()?;
        let current = pointer::<Context>(current)?;
        let next = pointer::<Context>(next)?;
        // SAFETY: Both contexts must be exclusive and initialized, and the target frame and its stack must remain alive and mapped.
        unsafe {
            activate(
                current,
                next,
                &mut *core::ptr::addr_of_mut!(trap::STATE),
                ContextStatus::Retired,
            )?;
            restore(next)
        }
    }
}

fn user_context(storage: &ContextStorage) -> Result<*mut Context, ExecutionError> {
    let context = pointer::<Context>(storage)?;
    // SAFETY: The context and entry state must be exclusively held by the current processor with interrupts disabled.
    unsafe {
        validate_user_access(
            context,
            (*context).status,
            &*core::ptr::addr_of!(trap::STATE),
        )?;
        if (*context).frame.status & SPP != 0 {
            return Err(ExecutionError::InvalidState);
        }
    }
    Ok(context)
}

fn check_environment() -> Result<(), ExecutionError> {
    if CPU.interrupt_state() != InterruptState::Disabled {
        return Err(ExecutionError::InterruptsEnabled);
    }
    let status: usize;
    // SAFETY: The current processor must allow reading sstatus.
    unsafe {
        asm!("csrr {}, sstatus", out(reg) status, options(nomem, nostack));
    }
    if status & (FS | VS) != 0 {
        return Err(ExecutionError::ExtensionsEnabled);
    }
    // SAFETY: The entry state must be exclusively held by the current processor, and interrupts must be disabled during access.
    let state = unsafe { &*core::ptr::addr_of!(trap::STATE) };
    if state.handler.is_none() {
        return Err(ExecutionError::NotInstalled);
    }
    if state.active > 1 {
        return Err(ExecutionError::InvalidState);
    }
    Ok(())
}

pub(super) extern "C" fn capture(frame: *mut TrapFrame) -> *mut TrapFrame {
    // SAFETY: frame must be the complete frame of the current entry, and the entry state and current context must be exclusively held by the current processor.
    unsafe { capture_frame(frame, &*core::ptr::addr_of!(trap::STATE)) }
}

/// # Safety
///
/// Both contexts and their stacks must remain alive and mapped, and interrupts and floating point must be disabled.
#[unsafe(naked)]
unsafe extern "C" fn switch(_current: *mut KernelContinuation, _next: *const Context) {
    naked_asm!(
        "sd ra, 0(a0)",
        "sd sp, 8(a0)",
        "sd gp, 16(a0)",
        "sd tp, 24(a0)",
        "sd s0, 32(a0)",
        "sd s1, 40(a0)",
        "sd s2, 48(a0)",
        "sd s3, 56(a0)",
        "sd s4, 64(a0)",
        "sd s5, 72(a0)",
        "sd s6, 80(a0)",
        "sd s7, 88(a0)",
        "sd s8, 96(a0)",
        "sd s9, 104(a0)",
        "sd s10, 112(a0)",
        "sd s11, 120(a0)",
        "mv a0, a1",
        "tail {restore}",
        restore = sym restore,
    );
}

// REFERENCE: https://docs.riscv.org/reference/isa/unpriv/zalrsc.html

/// # Safety
///
/// The target frame and its stack must be accessible, the entry state must already match the target, and interrupts and floating point must be disabled.
#[unsafe(naked)]
unsafe extern "C" fn restore(_next: *const Context) -> ! {
    naked_asm!(
        ".option push",
        ".option arch, +a",
        "sc.d zero, zero, (a0)",
        "addi a0, a0, {continuation}",
        "ld ra, 0(a0)",
        "ld sp, 8(a0)",
        "ld gp, 16(a0)",
        "ld tp, 24(a0)",
        "ld s0, 32(a0)",
        "ld s1, 40(a0)",
        "ld s2, 48(a0)",
        "ld s3, 56(a0)",
        "ld s4, 64(a0)",
        "ld s5, 72(a0)",
        "ld s6, 80(a0)",
        "ld s7, 88(a0)",
        "ld s8, 96(a0)",
        "ld s9, 104(a0)",
        "ld s10, 112(a0)",
        "ld s11, 120(a0)",
        "ret",
        ".option pop",
        continuation = const offset_of!(Context, continuation),
    );
}

#[unsafe(naked)]
extern "C" fn start_kernel() -> ! {
    naked_asm!("mv a0, s1", "jr s0",);
}

#[unsafe(naked)]
extern "C" fn start_user() -> ! {
    naked_asm!(
        "mv sp, s0",
        "mv a0, sp",
        "tail {restore}",
        restore = sym restore_frame,
    );
}

#[unsafe(naked)]
pub(super) extern "C" fn save_floating(_frame: &mut TrapFrame) {
    naked_asm!(
        ".option push",
        ".option arch, +f, +d",
        "ld t0, {status}(a0)",
        "li t1, 0x6000",
        "and t0, t0, t1",
        "beqz t0, 2f",
        "frcsr t0",
        "sd t0, {fcsr}(a0)",
        "addi a0, a0, {floating}",
        "fsd f0, 0(a0)",
        "fsd f1, 8(a0)",
        "fsd f2, 16(a0)",
        "fsd f3, 24(a0)",
        "fsd f4, 32(a0)",
        "fsd f5, 40(a0)",
        "fsd f6, 48(a0)",
        "fsd f7, 56(a0)",
        "fsd f8, 64(a0)",
        "fsd f9, 72(a0)",
        "fsd f10, 80(a0)",
        "fsd f11, 88(a0)",
        "fsd f12, 96(a0)",
        "fsd f13, 104(a0)",
        "fsd f14, 112(a0)",
        "fsd f15, 120(a0)",
        "fsd f16, 128(a0)",
        "fsd f17, 136(a0)",
        "fsd f18, 144(a0)",
        "fsd f19, 152(a0)",
        "fsd f20, 160(a0)",
        "fsd f21, 168(a0)",
        "fsd f22, 176(a0)",
        "fsd f23, 184(a0)",
        "fsd f24, 192(a0)",
        "fsd f25, 200(a0)",
        "fsd f26, 208(a0)",
        "fsd f27, 216(a0)",
        "fsd f28, 224(a0)",
        "fsd f29, 232(a0)",
        "fsd f30, 240(a0)",
        "fsd f31, 248(a0)",
        "2:",
        "li t0, 0x6000",
        "csrc sstatus, t0",
        "ret",
        ".option pop",
        status = const offset_of!(TrapFrame, status),
        fcsr = const offset_of!(TrapFrame, fcsr),
        floating = const offset_of!(TrapFrame, floating),
    );
}

#[unsafe(naked)]
extern "C" fn restore_floating(_frame: &TrapFrame) {
    naked_asm!(
        ".option push",
        ".option arch, +f, +d",
        "ld t0, {status}(a0)",
        "li t1, 0x6000",
        "and t0, t0, t1",
        "csrc sstatus, t1",
        "csrs sstatus, t0",
        "beqz t0, 2f",
        "ld t0, {fcsr}(a0)",
        "fscsr t0",
        "addi a0, a0, {floating}",
        "fld f0, 0(a0)",
        "fld f1, 8(a0)",
        "fld f2, 16(a0)",
        "fld f3, 24(a0)",
        "fld f4, 32(a0)",
        "fld f5, 40(a0)",
        "fld f6, 48(a0)",
        "fld f7, 56(a0)",
        "fld f8, 64(a0)",
        "fld f9, 72(a0)",
        "fld f10, 80(a0)",
        "fld f11, 88(a0)",
        "fld f12, 96(a0)",
        "fld f13, 104(a0)",
        "fld f14, 112(a0)",
        "fld f15, 120(a0)",
        "fld f16, 128(a0)",
        "fld f17, 136(a0)",
        "fld f18, 144(a0)",
        "fld f19, 152(a0)",
        "fld f20, 160(a0)",
        "fld f21, 168(a0)",
        "fld f22, 176(a0)",
        "fld f23, 184(a0)",
        "fld f24, 192(a0)",
        "fld f25, 200(a0)",
        "fld f26, 208(a0)",
        "fld f27, 216(a0)",
        "fld f28, 224(a0)",
        "fld f29, 232(a0)",
        "fld f30, 240(a0)",
        "fld f31, 248(a0)",
        "2:",
        "ret",
        ".option pop",
        status = const offset_of!(TrapFrame, status),
        fcsr = const offset_of!(TrapFrame, fcsr),
        floating = const offset_of!(TrapFrame, floating),
    );
}

/// # Safety
///
/// sp and the first argument must point to the same valid frame, the entry state must already match the target, and interrupts must be disabled.
#[unsafe(naked)]
pub(super) unsafe extern "C" fn restore_frame(_frame: &TrapFrame) -> ! {
    naked_asm!(
        ".option push",
        ".option arch, +a",
        "call {restore_floating}",
        // Clear the LR/SC reservation left by the previous context; the written x0 slot is always zero.
        "sc.d zero, zero, (sp)",
        "ld t0, {pc}(sp)",
        "csrw sepc, t0",
        "ld t0, {status}(sp)",
        "csrw sstatus, t0",
        "ld ra, 8(sp)",
        "ld gp, 24(sp)",
        "ld tp, 32(sp)",
        "ld t1, 48(sp)",
        "ld t2, 56(sp)",
        "ld s0, 64(sp)",
        "ld s1, 72(sp)",
        "ld a0, 80(sp)",
        "ld a1, 88(sp)",
        "ld a2, 96(sp)",
        "ld a3, 104(sp)",
        "ld a4, 112(sp)",
        "ld a5, 120(sp)",
        "ld a6, 128(sp)",
        "ld a7, 136(sp)",
        "ld s2, 144(sp)",
        "ld s3, 152(sp)",
        "ld s4, 160(sp)",
        "ld s5, 168(sp)",
        "ld s6, 176(sp)",
        "ld s7, 184(sp)",
        "ld s8, 192(sp)",
        "ld s9, 200(sp)",
        "ld s10, 208(sp)",
        "ld s11, 216(sp)",
        "ld t3, 224(sp)",
        "ld t4, 232(sp)",
        "ld t5, 240(sp)",
        "ld t6, 248(sp)",
        "ld t0, 40(sp)",
        "ld sp, 16(sp)",
        "sret",
        ".option pop",
        pc = const offset_of!(TrapFrame, pc),
        status = const offset_of!(TrapFrame, status),
        restore_floating = sym restore_floating,
    );
}
