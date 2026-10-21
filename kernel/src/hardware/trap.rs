// SPDX-License-Identifier: MPL-2.0

use crate::{
    memory::VirtRange,
    trap::{Trap, TrapOutcome},
};

use super::{BindError, Slot};

static SLOT: Slot<&'static dyn TrapControl> = Slot::new();

pub type Handler = fn(Trap) -> TrapOutcome;

pub fn bind(trap: &'static dyn TrapControl) -> Result<(), BindError> {
    SLOT.bind(trap)
}

pub fn trap() -> &'static dyn TrapControl {
    SLOT.get()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrapError {
    AlreadyInstalled,
    NotInstalled,
    InvalidStack,
    Busy,
    InterruptsEnabled,
}

/// # Safety
///
/// 安装及后续操作须在同一处理器执行；栈须独占、常驻且完整映射为内核可写内存，直到替换或停机。
/// handler 须有界执行，不展开栈、不启用中断或阻塞。
pub trait TrapControl: Sync {
    fn install(&self, stack: VirtRange, handler: Handler) -> Result<(), TrapError>;

    fn set_stack(&self, stack: VirtRange) -> Result<(), TrapError>;
}
