// SPDX-License-Identifier: MPL-2.0

use kernel::{
    hardware::trap::{Handler, TrapError},
    memory::VirtRange,
};

pub(crate) const EMERGENCY_STACK_SIZE: usize = 16 * 1024;
pub(crate) const STACK_HEADROOM: usize = 4096;

#[repr(C, align(16))]
pub(crate) struct EmergencyStack(pub [u8; EMERGENCY_STACK_SIZE]);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(usize)]
pub(crate) enum EntryReason {
    Normal,
    KernelException,
    InvalidStack,
    Reentrant,
}

#[repr(C)]
pub(crate) struct EntryState {
    pub stack_start: usize,
    pub stack_end: usize,
    pub emergency_top: usize,
    // 0: outside the entry; 1: normal handling; 2: emergency handling, another exception halts directly.
    pub active: usize,
    pub scratch: [usize; 4],
    pub kernel_tp: usize,
    pub kernel_gp: usize,
    pub handler: Option<Handler>,
    pub context: *mut (),
}

impl EntryState {
    pub const fn new() -> Self {
        Self {
            stack_start: 0,
            stack_end: 0,
            emergency_top: 0,
            active: 0,
            scratch: [0; 4],
            kernel_tp: 0,
            kernel_gp: 0,
            handler: None,
            context: core::ptr::null_mut(),
        }
    }

    pub fn set_stack(&mut self, stack: VirtRange, frame_size: usize) -> Result<(), TrapError> {
        if self.active != 0 {
            return Err(TrapError::Busy);
        }
        Self::validate_stack(stack, frame_size)?;
        self.stack_start = stack.start;
        self.stack_end = stack.end;
        Ok(())
    }

    pub fn validate_stack(stack: VirtRange, frame_size: usize) -> Result<(), TrapError> {
        if stack.start <= isize::MAX as usize
            || !stack.start.is_multiple_of(16)
            || !stack.end.is_multiple_of(16)
            || stack
                .end
                .checked_sub(stack.start)
                .is_none_or(|length| length < frame_size + STACK_HEADROOM)
        {
            return Err(TrapError::InvalidStack);
        }
        Ok(())
    }
}
