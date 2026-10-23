// SPDX-License-Identifier: MPL-2.0

use core::alloc::Layout;

use kernel::{
    hardware::{
        self,
        execution::{ContextStart, ContextStorage, ExecutionError},
    },
    memory::VirtRange,
};

use crate::trap::EntryState;

#[repr(C)]
pub(crate) struct SavedContext<F, C> {
    pub frame: F,
    // 内核调用续点不得覆盖挂起 handler 的用户现场。
    pub continuation: C,
    pub entry: EntryContext,
    pub status: ContextStatus,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum ContextStatus {
    Running,
    Suspended,
    Retired,
}

impl ContextStatus {
    pub fn initial(start: ContextStart) -> Self {
        if matches!(start, ContextStart::Current) {
            Self::Running
        } else {
            Self::Suspended
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct EntryContext {
    stack: VirtRange,
    active: usize,
}

impl EntryContext {
    pub fn capture(state: &EntryState) -> Self {
        Self {
            stack: VirtRange {
                start: state.stack_start,
                end: state.stack_end,
            },
            active: state.active,
        }
    }

    pub fn apply(self, state: &mut EntryState) {
        state.stack_start = self.stack.start;
        state.stack_end = self.stack.end;
        state.active = self.active;
    }

    pub fn initial(
        start: ContextStart,
        frame_size: usize,
        pc_alignment: usize,
    ) -> Result<Self, ExecutionError> {
        let stack = match start {
            ContextStart::Current => {
                return Ok(Self {
                    stack: VirtRange { start: 0, end: 0 },
                    active: 0,
                });
            }
            ContextStart::Kernel { stack, entry, .. } => {
                if !valid_address(entry as usize, false)
                    || !(entry as usize).is_multiple_of(pc_alignment)
                {
                    return Err(ExecutionError::InvalidEntry);
                }
                stack
            }
            ContextStart::User { stack, entry } => {
                if entry.pc == 0
                    || !valid_address(entry.pc, true)
                    || !entry.pc.is_multiple_of(pc_alignment)
                    || !valid_address(entry.thread_pointer, true)
                {
                    return Err(ExecutionError::InvalidEntry);
                }
                if entry.stack_pointer == 0
                    || !valid_address(entry.stack_pointer, true)
                    || !entry.stack_pointer.is_multiple_of(16)
                {
                    return Err(ExecutionError::InvalidStack);
                }
                stack
            }
        };
        if !valid_address(stack.start, false) || !valid_address(stack.end, false) {
            return Err(ExecutionError::InvalidStack);
        }
        EntryState::validate_stack(stack, frame_size).map_err(|_| ExecutionError::InvalidStack)?;
        Ok(Self { stack, active: 0 })
    }
}

pub(crate) fn pointer<T>(storage: &ContextStorage) -> Result<*mut T, ExecutionError> {
    if storage.layout() != Layout::new::<T>() {
        return Err(ExecutionError::InvalidLayout);
    }
    Ok(storage.address() as *mut T)
}

/// # Safety
///
/// 两份上下文须独占且已初始化，入口状态须由当前处理器在关中断时独占。
pub(crate) unsafe fn activate<F, C>(
    current: *mut SavedContext<F, C>,
    next: *mut SavedContext<F, C>,
    state: &mut EntryState,
    leaving: ContextStatus,
) -> Result<(), ExecutionError> {
    if current.is_null() || current == next || state.context != current.cast() {
        return Err(ExecutionError::InvalidState);
    }
    // SAFETY: 当前和目标存储须有效且互不重叠。
    unsafe {
        if (*current).status != ContextStatus::Running || (*next).status != ContextStatus::Suspended
        {
            return Err(ExecutionError::InvalidState);
        }
        (*current).entry = EntryContext::capture(state);
        (*next).entry.apply(state);
        (*current).status = leaving;
        (*next).status = ContextStatus::Running;
    }
    state.context = next.cast();
    Ok(())
}

/// # Safety
///
/// frame 须为当前入口的有效现场，state.context 须为空或指向以 F 起始的独占上下文存储。
pub(crate) unsafe fn capture_frame<F>(frame: *mut F, state: &EntryState) -> *mut F {
    if state.active != 1 || state.context.is_null() {
        return frame;
    }
    let destination = state.context.cast::<F>();
    // SAFETY: 两份现场须有效且不重叠，入口状态须由当前处理器独占。
    unsafe { frame.copy_to_nonoverlapping(destination, 1) };
    destination
}

fn valid_address(address: usize, user: bool) -> bool {
    (address <= isize::MAX as usize) == user && hardware::mmu().is_valid_virtual_address(address)
}
