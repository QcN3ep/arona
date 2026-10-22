// SPDX-License-Identifier: MPL-2.0

use super::{BindError, Slot};

static SLOT: Slot<&'static dyn Timer> = Slot::new();

pub fn bind(timer: &'static dyn Timer) -> Result<(), BindError> {
    SLOT.bind(timer)
}

pub fn timer() -> &'static dyn Timer {
    SLOT.get()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimerError {
    NotInitialized,
    AlreadyInitialized,
    InterruptsEnabled,
    Busy,
    InvalidFrequency,
    Unsupported,
    DeadlineOutOfRange,
    Firmware(isize),
}

/// # Safety
///
/// 所有操作须在同一处理器且关中断时执行；设置 deadline 前，异常入口及其栈须已就绪。
pub trait Timer: Sync {
    /// 初始化后时间从零开始，定时中断保持停用；无硬件频率来源时使用传入值。
    fn initialize(&self, frequency_hz: Option<u64>) -> Result<(), TimerError>;

    fn frequency_hz(&self) -> Result<u64, TimerError>;

    /// 返回自初始化起不回退的纳秒数，超出范围时饱和。
    fn now_ns(&self) -> Result<u64, TimerError>;

    /// 替换绝对纳秒 deadline，向上取整；已到期时尽快触发，失败保留原定时设置。
    fn set_deadline(&self, deadline_ns: u64) -> Result<(), TimerError>;

    /// 幂等停用定时中断，返回后旧 deadline 不再触发。
    fn cancel(&self) -> Result<(), TimerError>;

    /// 清除当前中断并保持停用。
    fn acknowledge(&self) -> Result<(), TimerError>;
}
