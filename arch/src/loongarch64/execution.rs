// SPDX-License-Identifier: MPL-2.0

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

crate::declare_hardware!(static EXECUTION: LoongArchExecution = LoongArchExecution => kernel::hardware::execution::bind);

type Context = SavedContext<TrapFrame, KernelContinuation>;

#[derive(Default)]
#[repr(C)]
struct KernelContinuation {
    ra: usize,
    tp: usize,
    sp: usize,
    r21: usize,
    fp: usize,
    saved: [usize; 9],
}

struct LoongArchExecution;

impl Execution for LoongArchExecution {
    fn context_layout(&self) -> Layout {
        Layout::new::<Context>()
    }

    fn user_entry(&self, state: &UserState) -> Result<UserEntry, ExecutionError> {
        if state.registers[0] != 0
            || state.floating_control & !0x1f1f_031f != 0
            || state
                .floating_conditions
                .iter()
                .any(|condition| *condition > 1)
        {
            return Err(ExecutionError::InvalidState);
        }
        validate_user_entry(
            UserEntry {
                pc: state.pc,
                stack_pointer: state.registers[3],
                thread_pointer: state.registers[2],
                argument: state.registers[4],
            },
            4,
        )
    }

    fn user_state(&self, storage: &ContextStorage) -> Result<UserState, ExecutionError> {
        check_environment()?;
        let context = user_context(storage)?;
        // SAFETY: 上下文须由当前处理器在关中断时独占。
        let frame = unsafe { &(*context).frame };
        Ok(UserState {
            pc: frame.pc,
            registers: frame.registers,
            floating: if frame.extensions & 1 != 0 {
                // SAFETY: FPU 启用时入口须已保存全部浮点寄存器。
                unsafe { frame.floating.assume_init() }
            } else {
                [0; 32]
            },
            floating_control: frame.fcsr as u32,
            floating_conditions: frame.fcc.to_le_bytes(),
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
        // SAFETY: 上下文须由当前处理器在关中断时独占。
        let frame = unsafe { &mut (*context).frame };
        frame.pc = state.pc;
        frame.registers = state.registers;
        frame.floating = MaybeUninit::new(state.floating);
        frame.fcsr = state.floating_control as usize;
        frame.fcc = usize::from_le_bytes(state.floating_conditions);
        frame.extensions = 1;
        Ok(())
    }

    fn initialize(
        &self,
        storage: &ContextStorage,
        start: ContextStart,
    ) -> Result<(), ExecutionError> {
        check_environment()?;
        let destination = pointer::<Context>(storage)?;
        // SAFETY: 入口状态须由当前处理器在关中断时独占。
        let state = unsafe { &mut *core::ptr::addr_of_mut!(trap::STATE) };
        if matches!(start, ContextStart::Current) && !state.context.is_null() {
            return Err(ExecutionError::InvalidState);
        }
        let entry = EntryContext::initial(start, size_of::<TrapFrame>(), 4)?;
        let tp: usize;
        // SAFETY: 当前处理器须遵循内核 ABI。
        unsafe {
            asm!("move {}, $tp", out(reg) tp, options(nomem, nostack));
        }
        let mut frame = TrapFrame {
            registers: [0; 32],
            pc: 0,
            status: 0,
            cause: 0,
            address: 0,
            instruction: 0,
            interrupt_mask: 0,
            extensions: 0,
            fcsr: 0,
            fcc: 0,
            reason: crate::trap::EntryReason::Normal,
            floating: MaybeUninit::new([0; 32]),
        };
        let mut continuation = KernelContinuation {
            tp,
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
                frame.pc = entry.pc;
                frame.status = 3 | (1 << 2);
                frame.extensions = 1;
                frame.registers[2] = entry.thread_pointer;
                frame.registers[3] = entry.stack_pointer;
                frame.registers[4] = entry.argument;
            }
        }
        // SAFETY: destination 须为该上下文独占且按声明布局分配的稳定存储。
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
        // SAFETY: 两份上下文须独占且已初始化，入口状态不得并发访问。
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
        // SAFETY: 当前上下文及目标存储须存活，入口状态须由当前处理器独占。
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
        // SAFETY: 两份上下文须独占且已初始化，目标现场及其栈须保持存活和映射。
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
    // SAFETY: 上下文和入口状态须由当前处理器在关中断时独占。
    unsafe {
        validate_user_access(
            context,
            (*context).status,
            &*core::ptr::addr_of!(trap::STATE),
        )?;
        if (*context).frame.status & 3 != 3 {
            return Err(ExecutionError::InvalidState);
        }
    }
    Ok(context)
}

fn check_environment() -> Result<(), ExecutionError> {
    if CPU.interrupt_state() != InterruptState::Disabled {
        return Err(ExecutionError::InterruptsEnabled);
    }
    let extensions: usize;
    // SAFETY: 当前处理器须允许读取 EUEN。
    unsafe {
        asm!("csrrd {}, 0x2", out(reg) extensions, options(nomem, nostack));
    }
    if extensions & 7 != 0 {
        return Err(ExecutionError::ExtensionsEnabled);
    }
    // SAFETY: 入口状态须由当前处理器独占，访问期间中断须关闭。
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
    // SAFETY: frame 须为当前入口的完整现场，入口状态和当前上下文须由当前处理器独占。
    unsafe { capture_frame(frame, &*core::ptr::addr_of!(trap::STATE)) }
}

/// # Safety
///
/// 两份上下文及其栈须保持存活和映射，中断与浮点须关闭。
#[unsafe(naked)]
unsafe extern "C" fn switch(_current: *mut KernelContinuation, _next: *const Context) {
    naked_asm!(
        "st.d $ra, $a0, 0",
        "st.d $tp, $a0, 8",
        "st.d $sp, $a0, 16",
        "st.d $r21, $a0, 24",
        "st.d $fp, $a0, 32",
        "st.d $s0, $a0, 40",
        "st.d $s1, $a0, 48",
        "st.d $s2, $a0, 56",
        "st.d $s3, $a0, 64",
        "st.d $s4, $a0, 72",
        "st.d $s5, $a0, 80",
        "st.d $s6, $a0, 88",
        "st.d $s7, $a0, 96",
        "st.d $s8, $a0, 104",
        "move $a0, $a1",
        "b {restore}",
        restore = sym restore,
    );
}

// REFERENCE: https://loongson.github.io/LoongArch-Documentation/LoongArch-Vol1-EN.html

/// # Safety
///
/// 目标现场及其栈须可访问，入口状态须已匹配目标，中断与浮点须关闭。
#[unsafe(naked)]
unsafe extern "C" fn restore(_next: *const Context) -> ! {
    naked_asm!(
        "li.w $t0, 2",
        "csrwr $t0, 0x60",
        "addi.d $a0, $a0, {continuation}",
        "ld.d $ra, $a0, 0",
        "ld.d $tp, $a0, 8",
        "ld.d $sp, $a0, 16",
        "ld.d $r21, $a0, 24",
        "ld.d $fp, $a0, 32",
        "ld.d $s0, $a0, 40",
        "ld.d $s1, $a0, 48",
        "ld.d $s2, $a0, 56",
        "ld.d $s3, $a0, 64",
        "ld.d $s4, $a0, 72",
        "ld.d $s5, $a0, 80",
        "ld.d $s6, $a0, 88",
        "ld.d $s7, $a0, 96",
        "ld.d $s8, $a0, 104",
        "jirl $zero, $ra, 0",
        continuation = const offset_of!(Context, continuation),
    );
}

#[unsafe(naked)]
extern "C" fn start_kernel() -> ! {
    naked_asm!("move $a0, $s1", "jirl $zero, $s0, 0",);
}

#[unsafe(naked)]
extern "C" fn start_user() -> ! {
    naked_asm!(
        "move $sp, $s0",
        "move $a0, $sp",
        "b {restore}",
        restore = sym restore_frame,
    );
}

#[unsafe(naked)]
pub(super) extern "C" fn save_floating(_frame: &mut TrapFrame) {
    naked_asm!(
        "ld.d $t0, $a0, {extensions}",
        "andi $t0, $t0, 1",
        "beqz $t0, 2f",
        "movfcsr2gr $t0, $fcsr0",
        "st.d $t0, $a0, {fcsr}",
        "movcf2gr $t0, $fcc0",
        "st.b $t0, $a0, {fcc}+0",
        "movcf2gr $t0, $fcc1",
        "st.b $t0, $a0, {fcc}+1",
        "movcf2gr $t0, $fcc2",
        "st.b $t0, $a0, {fcc}+2",
        "movcf2gr $t0, $fcc3",
        "st.b $t0, $a0, {fcc}+3",
        "movcf2gr $t0, $fcc4",
        "st.b $t0, $a0, {fcc}+4",
        "movcf2gr $t0, $fcc5",
        "st.b $t0, $a0, {fcc}+5",
        "movcf2gr $t0, $fcc6",
        "st.b $t0, $a0, {fcc}+6",
        "movcf2gr $t0, $fcc7",
        "st.b $t0, $a0, {fcc}+7",
        "addi.d $a0, $a0, {floating}",
        "fst.d $f0, $a0, 0",
        "fst.d $f1, $a0, 8",
        "fst.d $f2, $a0, 16",
        "fst.d $f3, $a0, 24",
        "fst.d $f4, $a0, 32",
        "fst.d $f5, $a0, 40",
        "fst.d $f6, $a0, 48",
        "fst.d $f7, $a0, 56",
        "fst.d $f8, $a0, 64",
        "fst.d $f9, $a0, 72",
        "fst.d $f10, $a0, 80",
        "fst.d $f11, $a0, 88",
        "fst.d $f12, $a0, 96",
        "fst.d $f13, $a0, 104",
        "fst.d $f14, $a0, 112",
        "fst.d $f15, $a0, 120",
        "fst.d $f16, $a0, 128",
        "fst.d $f17, $a0, 136",
        "fst.d $f18, $a0, 144",
        "fst.d $f19, $a0, 152",
        "fst.d $f20, $a0, 160",
        "fst.d $f21, $a0, 168",
        "fst.d $f22, $a0, 176",
        "fst.d $f23, $a0, 184",
        "fst.d $f24, $a0, 192",
        "fst.d $f25, $a0, 200",
        "fst.d $f26, $a0, 208",
        "fst.d $f27, $a0, 216",
        "fst.d $f28, $a0, 224",
        "fst.d $f29, $a0, 232",
        "fst.d $f30, $a0, 240",
        "fst.d $f31, $a0, 248",
        "2:",
        "addi.d $t0, $zero, 1",
        "csrxchg $zero, $t0, 0x2",
        "jirl $zero, $ra, 0",
        extensions = const offset_of!(TrapFrame, extensions),
        fcsr = const offset_of!(TrapFrame, fcsr),
        fcc = const offset_of!(TrapFrame, fcc),
        floating = const offset_of!(TrapFrame, floating),
    );
}

#[unsafe(naked)]
extern "C" fn restore_floating(_frame: &TrapFrame) {
    naked_asm!(
        "ld.d $t0, $a0, {extensions}",
        "move $t1, $t0",
        "csrwr $t1, 0x2",
        "andi $t0, $t0, 1",
        "beqz $t0, 2f",
        "ld.d $t0, $a0, {fcsr}",
        "movgr2fcsr $fcsr0, $t0",
        "ld.bu $t0, $a0, {fcc}+0",
        "movgr2cf $fcc0, $t0",
        "ld.bu $t0, $a0, {fcc}+1",
        "movgr2cf $fcc1, $t0",
        "ld.bu $t0, $a0, {fcc}+2",
        "movgr2cf $fcc2, $t0",
        "ld.bu $t0, $a0, {fcc}+3",
        "movgr2cf $fcc3, $t0",
        "ld.bu $t0, $a0, {fcc}+4",
        "movgr2cf $fcc4, $t0",
        "ld.bu $t0, $a0, {fcc}+5",
        "movgr2cf $fcc5, $t0",
        "ld.bu $t0, $a0, {fcc}+6",
        "movgr2cf $fcc6, $t0",
        "ld.bu $t0, $a0, {fcc}+7",
        "movgr2cf $fcc7, $t0",
        "addi.d $a0, $a0, {floating}",
        "fld.d $f0, $a0, 0",
        "fld.d $f1, $a0, 8",
        "fld.d $f2, $a0, 16",
        "fld.d $f3, $a0, 24",
        "fld.d $f4, $a0, 32",
        "fld.d $f5, $a0, 40",
        "fld.d $f6, $a0, 48",
        "fld.d $f7, $a0, 56",
        "fld.d $f8, $a0, 64",
        "fld.d $f9, $a0, 72",
        "fld.d $f10, $a0, 80",
        "fld.d $f11, $a0, 88",
        "fld.d $f12, $a0, 96",
        "fld.d $f13, $a0, 104",
        "fld.d $f14, $a0, 112",
        "fld.d $f15, $a0, 120",
        "fld.d $f16, $a0, 128",
        "fld.d $f17, $a0, 136",
        "fld.d $f18, $a0, 144",
        "fld.d $f19, $a0, 152",
        "fld.d $f20, $a0, 160",
        "fld.d $f21, $a0, 168",
        "fld.d $f22, $a0, 176",
        "fld.d $f23, $a0, 184",
        "fld.d $f24, $a0, 192",
        "fld.d $f25, $a0, 200",
        "fld.d $f26, $a0, 208",
        "fld.d $f27, $a0, 216",
        "fld.d $f28, $a0, 224",
        "fld.d $f29, $a0, 232",
        "fld.d $f30, $a0, 240",
        "fld.d $f31, $a0, 248",
        "2:",
        "jirl $zero, $ra, 0",
        extensions = const offset_of!(TrapFrame, extensions),
        fcsr = const offset_of!(TrapFrame, fcsr),
        fcc = const offset_of!(TrapFrame, fcc),
        floating = const offset_of!(TrapFrame, floating),
    );
}

/// # Safety
///
/// sp 与首参数须指向同一有效现场，入口状态须已匹配目标且中断须关闭。
#[unsafe(naked)]
pub(super) unsafe extern "C" fn restore_frame(_frame: &TrapFrame) -> ! {
    naked_asm!(
        "bl {restore_floating}",
        // WCLLB 写 1 清除 LLBit，同时保持 KLO 为 0。
        "li.w $t0, 2",
        "csrwr $t0, 0x60",
        "ld.d $t0, $sp, {pc}",
        "csrwr $t0, 0x6",
        "ld.d $t0, $sp, {status}",
        "csrwr $t0, 0x1",
        "ld.d $r1, $sp, 8",
        "ld.d $r2, $sp, 16",
        "ld.d $r4, $sp, 32",
        "ld.d $r5, $sp, 40",
        "ld.d $r6, $sp, 48",
        "ld.d $r7, $sp, 56",
        "ld.d $r8, $sp, 64",
        "ld.d $r9, $sp, 72",
        "ld.d $r10, $sp, 80",
        "ld.d $r11, $sp, 88",
        "ld.d $r13, $sp, 104",
        "ld.d $r14, $sp, 112",
        "ld.d $r15, $sp, 120",
        "ld.d $r16, $sp, 128",
        "ld.d $r17, $sp, 136",
        "ld.d $r18, $sp, 144",
        "ld.d $r19, $sp, 152",
        "ld.d $r20, $sp, 160",
        "ld.d $r21, $sp, 168",
        "ld.d $r22, $sp, 176",
        "ld.d $r23, $sp, 184",
        "ld.d $r24, $sp, 192",
        "ld.d $r25, $sp, 200",
        "ld.d $r26, $sp, 208",
        "ld.d $r27, $sp, 216",
        "ld.d $r28, $sp, 224",
        "ld.d $r29, $sp, 232",
        "ld.d $r30, $sp, 240",
        "ld.d $r31, $sp, 248",
        "ld.d $t0, $sp, 96",
        "ld.d $sp, $sp, 24",
        "ertn",
        pc = const offset_of!(TrapFrame, pc),
        status = const offset_of!(TrapFrame, status),
        restore_floating = sym restore_floating,
    );
}
