// SPDX-License-Identifier: MPL-2.0

//! 本地中断控制、处理器等待与停机能力。

use core::marker::PhantomData;

use super::{BindError, Slot};

static SLOT: Slot<&'static dyn Cpu> = Slot::new();

pub fn bind(cpu: &'static dyn Cpu) -> Result<(), BindError> {
    SLOT.bind(cpu)
}

pub fn cpu() -> &'static dyn Cpu {
    SLOT.get()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterruptState {
    Disabled,
    Enabled,
}

/// # Safety
///
/// 创建及析构须在同一处理器且允许控制本地中断；恢复中断前异常入口及其栈须已就绪。
#[must_use]
pub(crate) struct InterruptGuard {
    state: InterruptState,
    local: PhantomData<*mut ()>,
}

impl InterruptGuard {
    pub(crate) fn new() -> Self {
        Self {
            state: cpu().disable_interrupts(),
            local: PhantomData,
        }
    }
}

impl Drop for InterruptGuard {
    fn drop(&mut self) {
        cpu().restore_interrupts(self.state);
    }
}

/// 操作只影响当前处理器，不阻塞于锁或分配内存。
///
/// # Safety
///
/// 执行环境须允许控制本地中断与执行等待指令；启用中断前，异常入口及其栈须已就绪。
/// 调用 idle_until_interrupt 时须已关闭本地中断。
pub trait Cpu: Sync {
    fn interrupt_state(&self) -> InterruptState;

    /// 原子关闭中断并返回原状态；嵌套调用按逆序恢复。
    fn disable_interrupts(&self) -> InterruptState;

    /// 只恢复本地中断开关，保留其他处理器状态。
    fn restore_interrupts(&self, state: InterruptState);

    /// 在关中断状态等待并允许处理中断，返回时仍关中断；允许伪唤醒，不丢失已挂起的中断。
    fn idle_until_interrupt(&self);

    /// 关闭本地中断并永久停止当前控制流。
    fn halt(&self) -> !;
}
