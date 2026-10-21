// SPDX-License-Identifier: MPL-2.0

//! 独占上下文存储及其创建、恢复和切换能力。

use core::{alloc::Layout, cell::Cell, convert::Infallible};

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

#[derive(Clone, Copy, Debug)]
pub enum ContextStart {
    /// 为当前控制流提供保存位置，首次切出前不可恢复。
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
    state: Cell<ContextState>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ContextState {
    Running,
    Suspended,
    Retired,
}

impl Context {
    /// # Safety
    ///
    /// 上下文须始终在同一处理器使用；其栈、入口及用户地址空间须存活至停止执行，
    /// 用户栈与特权栈须独占，且栈和上下文存储在切换使用的页表中须保持映射。
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
        Ok(Self {
            storage,
            state: Cell::new(if matches!(start, ContextStart::Current) {
                ContextState::Running
            } else {
                ContextState::Suspended
            }),
        })
    }

    /// 返回表示当前控制流再次被恢复；失败时两个上下文均保持原状。
    ///
    /// # Safety
    ///
    /// 当前控制流须独占当前栈，切换时不得持有锁或跨线程借用；本地中断须关闭。
    /// 页表和当前线程的选择由调用方在切换前完成，离开的活动栈须由其他控制流回收。
    pub fn switch_to(&self, next: &Self) -> Result<(), ExecutionError> {
        check_interrupts()?;
        if core::ptr::eq(self, next)
            || self.state.get() != ContextState::Running
            || next.state.get() != ContextState::Suspended
        {
            return Err(ExecutionError::InvalidState);
        }
        self.state.set(ContextState::Suspended);
        next.state.set(ContextState::Running);
        match execution().switch(&self.storage, &next.storage) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.state.set(ContextState::Running);
                next.state.set(ContextState::Suspended);
                Err(error)
            }
        }
    }

    /// 放弃当前控制流并恢复 next；被放弃的上下文不再可用。
    ///
    /// # Safety
    ///
    /// 调用环境同 switch_to；当前控制流不再恢复，其活动栈须延后回收。
    pub fn restore(&self, next: &Self) -> Result<Infallible, ExecutionError> {
        check_interrupts()?;
        if core::ptr::eq(self, next)
            || self.state.get() != ContextState::Running
            || next.state.get() != ContextState::Suspended
        {
            return Err(ExecutionError::InvalidState);
        }
        self.state.set(ContextState::Retired);
        next.state.set(ContextState::Running);
        execution()
            .restore(&self.storage, &next.storage)
            .inspect_err(|_| {
                self.state.set(ContextState::Running);
                next.state.set(ContextState::Suspended);
            })
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
/// 实现者只能解释自身声明布局的存储；切换前须完成全部校验，失败不得修改入口状态或寄存器。
/// 切换不得保留指向 Context 包装对象的指针，且只能在同一处理器、关中断及标量浮点关闭时执行。
pub trait Execution: Sync {
    fn context_layout(&self) -> Layout;
    fn initialize(
        &self,
        storage: &ContextStorage,
        start: ContextStart,
    ) -> Result<(), ExecutionError>;
    fn switch(&self, current: &ContextStorage, next: &ContextStorage)
    -> Result<(), ExecutionError>;
    fn restore(
        &self,
        current: &ContextStorage,
        next: &ContextStorage,
    ) -> Result<Infallible, ExecutionError>;
}
