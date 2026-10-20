// SPDX-License-Identifier: MPL-2.0

//! 中断控制器到 CPU 的输出路由。

use super::HardwareDeviceInstanceId;

pub const MAX_CPU_INTERRUPT_ROUTES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuInterruptRoute {
    pub controller: HardwareDeviceInstanceId,
    /// 控制器输出编号；PLIC 使用上下文编号。
    pub output: u32,
    pub target: CpuInterruptTarget,
    pub line: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpuInterruptTarget {
    Cpu(u64),
    AllCpus,
}
