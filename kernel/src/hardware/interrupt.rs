// SPDX-License-Identifier: MPL-2.0

use core::marker::PhantomData;

use super::{BindError, Slot, mmio::MmioMapper};
use crate::{HardwareDeviceInstanceId, HardwareInfo, HardwareInterruptInfo};

static SLOT: Slot<&'static dyn InterruptController> = Slot::new();

pub fn bind(controller: &'static dyn InterruptController) -> Result<(), BindError> {
    SLOT.bind(controller)
}

pub fn interrupt_controller() -> &'static dyn InterruptController {
    SLOT.get()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterruptError {
    NotInitialized,
    AlreadyInitialized,
    InterruptsEnabled,
    Busy,
    InvalidSource,
    InvalidTopology,
    InvalidClaim,
    Unsupported,
    Memory(crate::memory::MemoryError),
}

impl From<crate::memory::MemoryError> for InterruptError {
    fn from(error: crate::memory::MemoryError) -> Self {
        Self::Memory(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InterruptSource {
    pub controller: HardwareDeviceInstanceId,
    pub line: u32,
}

impl From<HardwareInterruptInfo> for InterruptSource {
    fn from(info: HardwareInterruptInfo) -> Self {
        Self {
            controller: info.controller,
            line: info.line,
        }
    }
}

type Completion = fn(InterruptSource, usize, bool) -> Result<(), InterruptError>;

#[must_use]
#[derive(Debug)]
pub struct InterruptClaim {
    source: InterruptSource,
    token: usize,
    completion: Option<Completion>,
    local: PhantomData<*mut ()>,
}

impl InterruptClaim {
    pub fn new(source: InterruptSource, token: usize, completion: Completion) -> Self {
        Self {
            source,
            token,
            completion: Some(completion),
            local: PhantomData,
        }
    }

    pub fn source(&self) -> InterruptSource {
        self.source
    }

    pub fn complete(mut self) -> Result<(), InterruptError> {
        if let Some(complete) = self.completion {
            complete(self.source, self.token, false)?;
            self.completion = None;
        }
        Ok(())
    }
}

impl Drop for InterruptClaim {
    fn drop(&mut self) {
        // 未处理或处理失败的输入先屏蔽，再完成认领。
        if let Some(complete) = self.completion.take()
            && let Err(error) = complete(self.source, self.token, true)
        {
            log::error!("Failed to complete external interrupt: {error:?}");
            super::cpu().halt();
        }
    }
}

/// # Safety
///
/// 操作及认领对象析构须在初始化时的处理器且关中断时执行，异常入口及其栈须已就绪。
pub trait InterruptController: Sync {
    /// 初始化路由并屏蔽全部输入；硬件描述须对应当前平台。
    fn initialize(
        &self,
        hardware: &HardwareInfo,
        cpu_id: u64,
        mmio: &dyn MmioMapper,
    ) -> Result<(), InterruptError>;

    fn mask(&self, source: InterruptSource) -> Result<(), InterruptError>;

    /// 设置触发方式后开放输入；认领期间不得重新配置该输入。
    fn unmask(&self, interrupt: HardwareInterruptInfo) -> Result<(), InterruptError>;

    /// 一次只允许一个未完成的认领，无待处理输入时返回 None。
    fn claim(&self) -> Result<Option<InterruptClaim>, InterruptError>;
}
