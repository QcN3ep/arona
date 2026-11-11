// SPDX-License-Identifier: MPL-2.0

//! Exclusive context storage and its creation, restoration, and switching capability.

use core::{alloc::Layout, convert::Infallible};

use crate::memory::{self, HeapAllocation, MemoryError, VirtRange};

use super::{BindError, Slot, cpu::InterruptState};

const MAX_CONTEXT_SIZE: usize = 4096;

static SLOT: Slot<&'static dyn Execution> = Slot::new();

pub fn bind(execution: &'static dyn Execution) -> Result<(), BindError> {
    SLOT.bind(execution)
}

pub fn execution() -> &'static dyn Execution {
    SLOT.get()
}

pub type KernelEntry = extern "C" fn(usize) -> !;

#[derive(Clone, Copy, Debug)]
pub struct UserEntry {
    pub pc: usize,
    pub stack_pointer: usize,
    pub thread_pointer: usize,
    pub argument: usize,
}

pub use abi::trap::UserState;

#[derive(Clone, Copy, Debug)]
pub enum ContextStart {
    /// Provides a save location for the current control flow; cannot be restored before the first switch-out.
    Current,
    Kernel {
        stack: VirtRange,
        entry: KernelEntry,
        argument: usize,
    },
    User {
        stack: VirtRange,
        entry: UserEntry,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionError {
    InvalidLayout,
    InvalidEntry,
    InvalidStack,
    InvalidState,
    NotInstalled,
    InterruptsEnabled,
    ExtensionsEnabled,
    Memory(MemoryError),
}

pub struct ContextStorage {
    allocation: HeapAllocation,
}

impl ContextStorage {
    pub fn address(&self) -> usize {
        self.allocation.address()
    }

    pub fn layout(&self) -> Layout {
        self.allocation.layout()
    }
}

pub struct Context {
    storage: ContextStorage,
}

impl Context {
    pub fn user_state(&self) -> Result<UserState, ExecutionError> {
        check_interrupts()?;
        execution().user_state(&self.storage)
    }

    pub fn set_user_state(&self, state: &UserState) -> Result<(), ExecutionError> {
        check_interrupts()?;
        execution().set_user_state(&self.storage, state)
    }

    /// # Safety
    ///
    /// The context must always be used on the same processor; its stack, entry, and user address space must stay alive until execution stops,
    /// The user stack and privileged stack must be exclusive, and the stack and context storage must remain mapped in the page table used for switching.
    pub fn new(start: ContextStart) -> Result<Self, ExecutionError> {
        check_interrupts()?;
        let provider = execution();
        let layout = provider.context_layout();
        if layout.size() == 0
            || layout.size() > MAX_CONTEXT_SIZE
            || layout.align() > MAX_CONTEXT_SIZE
        {
            return Err(ExecutionError::InvalidLayout);
        }
        let storage = ContextStorage {
            allocation: memory::manager()
                .allocate_heap(layout)
                .map_err(ExecutionError::Memory)?,
        };
        provider.initialize(&storage, start)?;
        Ok(Self { storage })
    }

    /// Returns when the current control flow is resumed again; on failure both contexts remain unchanged.
    ///
    /// # Safety
    ///
    /// The current control flow must exclusively own the current stack, no locks or cross-thread borrows may be held while switching, and local interrupts must be disabled.
    /// The caller selects the page table and current Thread before switching; the departing active stack must be reclaimed by another control flow.
    pub fn switch_to(&self, next: &Self) -> Result<(), ExecutionError> {
        check_interrupts()?;
        execution().switch(&self.storage, &next.storage)
    }

    /// # Safety
    ///
    /// The execution environment is as for switch_to; the current context and its stack must be held by the resumer until the switch-out completes.
    pub fn switch_from_current(&self) -> Result<(), ExecutionError> {
        check_interrupts()?;
        execution().switch_current(&self.storage)
    }

    /// Abandons the current control flow and restores next; the abandoned context is no longer usable.
    ///
    /// # Safety
    ///
    /// The calling environment is as for switch_to; the current control flow is not resumed, and its active stack must be reclaimed later.
    pub fn restore(&self, next: &Self) -> Result<Infallible, ExecutionError> {
        check_interrupts()?;
        execution().restore(&self.storage, &next.storage)
    }
}

fn check_interrupts() -> Result<(), ExecutionError> {
    if super::cpu().interrupt_state() != InterruptState::Disabled {
        return Err(ExecutionError::InterruptsEnabled);
    }
    Ok(())
}

/// # Safety
///
/// Implementations may interpret only storage matching their declared layout; all validation must complete before switching, and failure must not modify entry state or registers.
/// A switch must not retain pointers to the Context wrapper object and may run only on the same processor with interrupts and scalar floating-point disabled.
pub trait Execution: Sync {
    fn context_layout(&self) -> Layout;
    fn user_entry(&self, state: &UserState) -> Result<UserEntry, ExecutionError>;
    fn user_state(&self, storage: &ContextStorage) -> Result<UserState, ExecutionError>;
    fn set_user_state(
        &self,
        storage: &ContextStorage,
        state: &UserState,
    ) -> Result<(), ExecutionError>;
    fn initialize(
        &self,
        storage: &ContextStorage,
        start: ContextStart,
    ) -> Result<(), ExecutionError>;
    fn switch(&self, current: &ContextStorage, next: &ContextStorage)
    -> Result<(), ExecutionError>;
    fn switch_current(&self, next: &ContextStorage) -> Result<(), ExecutionError>;
    fn restore(
        &self,
        current: &ContextStorage,
        next: &ContextStorage,
    ) -> Result<Infallible, ExecutionError>;
}
