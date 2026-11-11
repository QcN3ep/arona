// SPDX-License-Identifier: MPL-2.0

//! Output routing from interrupt controllers to CPUs.

use super::HardwareDeviceInstanceId;

pub const MAX_CPU_INTERRUPT_ROUTES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuInterruptRoute {
    pub controller: HardwareDeviceInstanceId,
    /// Controller output number; PLIC uses context numbers.
    pub output: u32,
    pub target: CpuInterruptTarget,
    pub line: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpuInterruptTarget {
    Cpu(u64),
    AllCpus,
}
