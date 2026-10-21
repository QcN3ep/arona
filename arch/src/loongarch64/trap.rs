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

crate::declare_hardware!(static TRAP: LoongArchTrap = LoongArchTrap => kernel::hardware::trap::bind);

pub(super) static mut STATE: EntryState = EntryState::new();
static mut EMERGENCY_STACK: EmergencyStack = EmergencyStack([0; EMERGENCY_STACK_SIZE]);

#[repr(C, align(16))]
pub(super) struct TrapFrame {
    pub(super) registers: [usize; 32],
    pub(super) pc: usize,
    pub(super) status: usize,
    pub(super) cause: usize,
    pub(super) address: usize,
    pub(super) instruction: usize,
    pub(super) interrupt_mask: usize,
    pub(super) extensions: usize,
    pub(super) fcsr: usize,
    pub(super) fcc: usize,
    pub(super) reason: EntryReason,
    pub(super) floating: MaybeUninit<[u64; 32]>,
}

struct LoongArchTrap;

impl TrapControl for LoongArchTrap {
    fn install(&self, stack: VirtRange, handler: Handler) -> Result<(), TrapError> {
        if CPU.interrupt_state() != InterruptState::Disabled {
            return Err(TrapError::InterruptsEnabled);
        }
        // SAFETY: 当前处理器须独占入口状态，并在关中断时安装。
        let state = unsafe { &mut *core::ptr::addr_of_mut!(STATE) };
        if state.handler.is_some() {
            return Err(TrapError::AlreadyInstalled);
        }
        state.set_stack(stack, size_of::<TrapFrame>())?;
        state.emergency_top = core::ptr::addr_of!(EMERGENCY_STACK) as usize + EMERGENCY_STACK_SIZE;
        state.handler = Some(handler);
        // SAFETY: 入口及其静态状态和栈须保持映射，当前处理器须独占 KS0。
        unsafe {
            asm!("move {}, $tp", out(reg) state.kernel_tp, options(nomem, nostack));
            asm!(
                "csrwr {state}, 0x30",
                "csrwr {entry}, 0xc",
                "li.w $t0, 0x70000",
                "csrxchg $zero, $t0, 0x4",
                state = inout(reg) core::ptr::addr_of!(STATE) as usize => _,
                entry = inout(reg) entry as *const () as usize => _,
                out("$t0") _,
                options(nostack),
            );
        }
        Ok(())
    }

    fn set_stack(&self, stack: VirtRange) -> Result<(), TrapError> {
        if CPU.interrupt_state() != InterruptState::Disabled {
            return Err(TrapError::InterruptsEnabled);
        }
        // SAFETY: 当前处理器须独占入口状态，且本地中断须关闭。
        let state = unsafe { &mut *core::ptr::addr_of_mut!(STATE) };
        if state.handler.is_none() {
            return Err(TrapError::NotInstalled);
        }
        state.set_stack(stack, size_of::<TrapFrame>())
    }
}

// REFERENCE: https://loongson.github.io/LoongArch-Documentation/LoongArch-Vol1-EN.html

/// # Safety
///
/// KS0 须指向当前处理器的常驻入口状态，入口代码及紧急栈须始终可访问。
#[unsafe(naked)]
#[unsafe(link_section = ".text.trap.entry")]
extern "C" fn entry() -> ! {
    naked_asm!(
        ".balign 64",
        "csrwr $t0, 0x30",
        "st.d $t1, $t0, {scratch}+8",
        "st.d $t2, $t0, {scratch}+16",
        "st.d $t3, $t0, {scratch}+24",
        "csrrd $t1, 0x30",
        "st.d $t1, $t0, {scratch}",
        "move $t1, $t0",
        "csrwr $t1, 0x30",
        "ld.d $t1, $t0, {active}",
        "addi.d $t2, $zero, 2",
        "beq $t1, $t2, 9f",
        "bnez $t1, 6f",
        "csrrd $t1, 0x1",
        "andi $t1, $t1, 3",
        "bnez $t1, 2f",
        "csrrd $t1, 0x5",
        "bstrpick.d $t1, $t1, 21, 16",
        "bnez $t1, 4f",
        "move $t1, $sp",
        "b 3f",
        "2:",
        "ld.d $t1, $t0, {end}",
        "3:",
        "andi $t2, $t1, 15",
        "bnez $t2, 5f",
        "ld.d $t2, $t0, {end}",
        "bltu $t2, $t1, 5f",
        "ld.d $t2, $t0, {start}",
        "bltu $t1, $t2, 5f",
        "sub.d $t2, $t1, $t2",
        "li.d $t3, {minimum}",
        "bltu $t2, $t3, 5f",
        "addi.d $t2, $zero, 1",
        "st.d $t2, $t0, {active}",
        "addi.d $t3, $zero, {normal}",
        "b 8f",
        "4:",
        "addi.d $t3, $zero, {kernel_exception}",
        "b 7f",
        "5:",
        "addi.d $t3, $zero, {invalid_stack}",
        "b 7f",
        "6:",
        "addi.d $t3, $zero, {reentrant}",
        "7:",
        "ld.d $t1, $t0, {emergency}",
        "addi.d $t2, $zero, 2",
        "st.d $t2, $t0, {active}",
        "8:",
        "addi.d $t1, $t1, -{frame_size}",
        "st.d $sp, $t1, 24",
        "move $sp, $t1",
        "st.d $t3, $sp, {reason}",
        "b {save_frame}",
        "9:",
        "b {double_fault}",
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
        "st.d $r0, $sp, 0",
        "st.d $r1, $sp, 8",
        "st.d $r2, $sp, 16",
        "st.d $r4, $sp, 32",
        "st.d $r5, $sp, 40",
        "st.d $r6, $sp, 48",
        "st.d $r7, $sp, 56",
        "st.d $r8, $sp, 64",
        "st.d $r9, $sp, 72",
        "st.d $r10, $sp, 80",
        "st.d $r11, $sp, 88",
        "st.d $r16, $sp, 128",
        "st.d $r17, $sp, 136",
        "st.d $r18, $sp, 144",
        "st.d $r19, $sp, 152",
        "st.d $r20, $sp, 160",
        "st.d $r21, $sp, 168",
        "st.d $r22, $sp, 176",
        "st.d $r23, $sp, 184",
        "st.d $r24, $sp, 192",
        "st.d $r25, $sp, 200",
        "st.d $r26, $sp, 208",
        "st.d $r27, $sp, 216",
        "st.d $r28, $sp, 224",
        "st.d $r29, $sp, 232",
        "st.d $r30, $sp, 240",
        "st.d $r31, $sp, 248",
        "ld.d $t1, $t0, {scratch}",
        "st.d $t1, $sp, 96",
        "ld.d $t1, $t0, {scratch}+8",
        "st.d $t1, $sp, 104",
        "ld.d $t1, $t0, {scratch}+16",
        "st.d $t1, $sp, 112",
        "ld.d $t1, $t0, {scratch}+24",
        "st.d $t1, $sp, 120",
        "csrrd $t1, 0x6",
        "st.d $t1, $sp, {pc}",
        "csrrd $t1, 0x1",
        "st.d $t1, $sp, {status}",
        "csrrd $t1, 0x5",
        "st.d $t1, $sp, {cause}",
        "csrrd $t1, 0x7",
        "st.d $t1, $sp, {address}",
        "csrrd $t1, 0x8",
        "st.d $t1, $sp, {instruction}",
        "csrrd $t1, 0x4",
        "st.d $t1, $sp, {interrupt_mask}",
        "csrrd $t1, 0x2",
        "st.d $t1, $sp, {extensions}",
        "st.d $zero, $sp, {fcsr}",
        "st.d $zero, $sp, {fcc}",
        "ld.d $tp, $t0, {tp}",
        "move $a0, $sp",
        "bl {save_floating}",
        "move $a0, $sp",
        "bl {capture}",
        "bl {handle}",
        "csrrd $t0, 0x30",
        "st.d $zero, $t0, {active}",
        "move $sp, $a0",
        "b {restore}",
        active = const offset_of!(EntryState, active),
        scratch = const offset_of!(EntryState, scratch),
        tp = const offset_of!(EntryState, kernel_tp),
        pc = const offset_of!(TrapFrame, pc),
        status = const offset_of!(TrapFrame, status),
        cause = const offset_of!(TrapFrame, cause),
        address = const offset_of!(TrapFrame, address),
        instruction = const offset_of!(TrapFrame, instruction),
        interrupt_mask = const offset_of!(TrapFrame, interrupt_mask),
        extensions = const offset_of!(TrapFrame, extensions),
        fcsr = const offset_of!(TrapFrame, fcsr),
        fcc = const offset_of!(TrapFrame, fcc),
        capture = sym execution::capture,
        save_floating = sym execution::save_floating,
        handle = sym handle,
        restore = sym execution::restore_frame,
    );
}

#[unsafe(naked)]
extern "C" fn double_fault() -> ! {
    naked_asm!(
        "li.w $t0, 4",
        "csrxchg $zero, $t0, 0x0",
        "2:",
        "idle 0",
        "b 2b"
    );
}

extern "C" fn handle(frame: &mut TrapFrame) -> *const TrapFrame {
    let trap = frame.decode();
    if frame.reason == EntryReason::Normal {
        // SAFETY: STATE 须已完成安装，访问期间须关闭本地中断。
        let handler = unsafe { (*core::ptr::addr_of!(STATE)).handler }.unwrap();
        match handler(trap) {
            TrapOutcome::Resume => return frame,
            TrapOutcome::Return { status, values } => {
                if let TrapEvent::Syscall { successor, .. } = trap.event {
                    frame.registers[4..7].copy_from_slice(&[status, values[0], values[1]]);
                    frame.pc = successor;
                    return frame;
                }
            }
            TrapOutcome::Stop => {}
        }
    }
    EARLY_CONSOLE.emergency_write(format_args!(
        "\nFATAL TRAP: {:?} at {:#x}: {:?}\nsp={:#x} cause={:#x} entry={:?}\n",
        trap.origin, trap.pc, trap.event, frame.registers[3], frame.cause, frame.reason,
    ));
    CPU.halt()
}

impl TrapFrame {
    fn decode(&self) -> Trap {
        let origin = if self.status & 3 == 0 {
            TrapOrigin::Kernel
        } else {
            TrapOrigin::User
        };
        let code = (self.cause >> 16) & 0x3f;
        let event = if code == 0 {
            let pending = self.cause & self.interrupt_mask & 0x1fff;
            if pending & (1 << 11) != 0 {
                TrapEvent::TimerInterrupt
            } else if pending & 0x3fc != 0 {
                TrapEvent::ExternalInterrupt
            } else if pending & 3 != 0 {
                TrapEvent::SoftwareInterrupt
            } else {
                TrapEvent::OtherInterrupt
            }
        } else {
            self.exception(code, origin)
        };
        Trap {
            origin,
            pc: self.pc,
            event,
        }
    }

    fn exception(&self, code: usize, origin: TrapOrigin) -> TrapEvent {
        match code {
            11 if origin == TrapOrigin::User => TrapEvent::Syscall {
                number: self.registers[11],
                arguments: self.registers[4..10].try_into().unwrap(),
                successor: self.pc + 4,
            },
            1..=7 => TrapEvent::PageFault {
                address: self.address,
                access: match code {
                    2 | 4 => FaultAccess::Write,
                    3 | 6 => FaultAccess::Execute,
                    7 => self.privilege_fault_access(),
                    _ => FaultAccess::Read,
                },
            },
            13 | 15..=17 => TrapEvent::IllegalInstruction {
                instruction: Some(self.instruction as u32),
            },
            _ => TrapEvent::OtherException {
                class: match code {
                    14 => ExceptionClass::Privilege,
                    8 => ExceptionClass::AccessFault,
                    9 => ExceptionClass::MisalignedAddress,
                    10 => ExceptionClass::BoundsCheck,
                    12 => ExceptionClass::Breakpoint,
                    18 => ExceptionClass::Arithmetic,
                    _ => ExceptionClass::Unknown,
                },
                address: match code {
                    8..=10 => Some(self.address),
                    _ => None,
                },
            },
        }
    }

    fn privilege_fault_access(&self) -> FaultAccess {
        if self.address == self.pc {
            return FaultAccess::Execute;
        }
        // PPI 不编码访存方向，按 BADI 区分标量存储与原子读改写指令。
        let instruction = self.instruction as u32;
        let store = matches!(instruction >> 24, 0x21 | 0x23 | 0x25 | 0x27)
            || matches!(instruction >> 22, 0xa4..=0xa7 | 0xad | 0xaf)
            || matches!(
                instruction >> 15,
                0x7020 | 0x7028 | 0x7030 | 0x7038 | 0x7070 | 0x7078
                    | 0x70ae | 0x70b0..=0x70e3 | 0x70ec..=0x70ef | 0x70f8..=0x70ff
            )
            || (instruction >> 15 == 0x70af && matches!((instruction >> 10) & 0x1f, 1 | 3));
        if store {
            FaultAccess::Write
        } else {
            FaultAccess::Read
        }
    }
}
