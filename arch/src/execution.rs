// SPDX-License-Identifier: MPL-2.0

use core::alloc::Layout;

use kernel::{
    hardware::{
        self,
        execution::{ContextStart, ContextStorage, ExecutionError, UserEntry},
    },
    memory::VirtRange,
};

use crate::trap::EntryState;

#[repr(C)]
pub(crate) struct SavedContext<F, C> {
    pub frame: F,
    // The kernel call continuation must not overwrite the user frame of a suspended handler.
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

pub(crate) fn validate_user_entry(
    entry: UserEntry,
    pc_alignment: usize,
) -> Result<UserEntry, ExecutionError> {
    if entry.pc == 0
        || !valid_address(entry.pc, true)
        || !entry.pc.is_multiple_of(pc_alignment)
        || !valid_address(entry.thread_pointer, true)
    {
        return Err(ExecutionError::InvalidEntry);
    }
    if entry.stack_pointer == 0 || !valid_address(entry.stack_pointer, true) {
        return Err(ExecutionError::InvalidStack);
    }
    Ok(entry)
}

pub(crate) fn validate_user_access<F, C>(
    context: *const SavedContext<F, C>,
    status: ContextStatus,
    state: &EntryState,
) -> Result<(), ExecutionError> {
    match status {
        ContextStatus::Suspended => Ok(()),
        ContextStatus::Running
            if state.context == context.cast_mut().cast() && state.active == 1 =>
        {
            Ok(())
        }
        _ => Err(ExecutionError::InvalidState),
    }
}

/// # Safety
///
/// Both contexts must be exclusive and initialized, and the entry state must be exclusively held by the current processor with interrupts disabled.
pub(crate) unsafe fn activate<F, C>(
    current: *mut SavedContext<F, C>,
    next: *mut SavedContext<F, C>,
    state: &mut EntryState,
    leaving: ContextStatus,
) -> Result<(), ExecutionError> {
    if current.is_null() || current == next || state.context != current.cast() {
        return Err(ExecutionError::InvalidState);
    }
    // SAFETY: The current and target storage must be valid and non-overlapping.
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
/// frame must be the valid frame of the current entry, and state.context must be null or point to exclusive context storage beginning with F.
pub(crate) unsafe fn capture_frame<F>(frame: *mut F, state: &EntryState) -> *mut F {
    if state.active != 1 || state.context.is_null() {
        return frame;
    }
    let destination = state.context.cast::<F>();
    // SAFETY: Both frames must be valid and non-overlapping, and the entry state must be exclusively held by the current processor.
    unsafe { frame.copy_to_nonoverlapping(destination, 1) };
    destination
}

fn valid_address(address: usize, user: bool) -> bool {
    (address <= isize::MAX as usize) == user && hardware::mmu().is_valid_virtual_address(address)
}
