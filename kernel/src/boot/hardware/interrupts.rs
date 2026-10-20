// SPDX-License-Identifier: MPL-2.0

//! 中断控制器到 CPU 的输出路由。

use super::{
    DeviceConfiguration, HardwareDeviceInstanceId, HardwareDeviceKind, HardwareInterruptInfo,
};

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

impl super::HardwareInfo {
    pub(super) fn validate_interrupts(&self) -> Result<(), &'static str> {
        self.validate_cpu_interrupts()?;
        for (index, device) in self.devices.iter().enumerate() {
            if let Some(route) = device.interrupt_route {
                let parent = self.device(route.controller)?;
                let end = match &device.configuration {
                    DeviceConfiguration::InterruptController { lines } => {
                        route.base_line.checked_add(lines.end)
                    }
                    DeviceConfiguration::Msi { vectors } if route.base_line == 0 => {
                        Some(vectors.end)
                    }
                    _ => None,
                };
                if parent.kind != HardwareDeviceKind::LOONGARCH_EIOINTC
                    || end.is_none_or(|end| end > 256)
                    || !matches!(
                        device.kind,
                        HardwareDeviceKind::LOONGARCH_PCH_PIC
                            | HardwareDeviceKind::LOONGARCH_PCH_MSI
                    )
                {
                    return Err("unsupported interrupt cascade");
                }
            }
            for (position, interrupt) in device.interrupts.iter().enumerate() {
                let source = self.interrupt_source(*interrupt)?;
                for other in self.devices[..index]
                    .iter()
                    .flat_map(|device| &device.interrupts)
                    .chain(&device.interrupts[..position])
                {
                    if self.interrupt_source(*other)? == source {
                        return Err("shared IRQs are not supported");
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn interrupt_source(
        &self,
        interrupt: HardwareInterruptInfo,
    ) -> Result<(HardwareDeviceInstanceId, u32), &'static str> {
        let controller = self.device(interrupt.controller)?;
        let DeviceConfiguration::InterruptController { lines } = &controller.configuration else {
            return Err("invalid interrupt controller");
        };
        if !lines.contains(&interrupt.line) {
            return Err("interrupt line is out of range");
        }
        if let Some(route) = controller.interrupt_route {
            if controller.kind != HardwareDeviceKind::LOONGARCH_PCH_PIC
                || self.device(route.controller)?.kind != HardwareDeviceKind::LOONGARCH_EIOINTC
            {
                return Err("unsupported interrupt cascade");
            }
            let line = route
                .base_line
                .checked_add(interrupt.line)
                .filter(|&line| line < 256)
                .ok_or("interrupt route is out of range")?;
            return Ok((route.controller, line));
        }
        Ok((interrupt.controller, interrupt.line))
    }

    pub(super) fn validate_cpu_interrupts(&self) -> Result<(), &'static str> {
        for (index, route) in self.cpu_interrupts.iter().enumerate() {
            let valid = match (self.device(route.controller)?.kind, route.target) {
                (HardwareDeviceKind::RISCV_PLIC, CpuInterruptTarget::Cpu(_)) => {
                    matches!(route.line, 9 | 11)
                }
                (HardwareDeviceKind::RISCV_CLINT, CpuInterruptTarget::Cpu(_)) => {
                    matches!(route.line, 3 | 7)
                }
                (HardwareDeviceKind::LOONGARCH_EIOINTC, CpuInterruptTarget::AllCpus) => {
                    route.line < 14
                }
                _ => false,
            };
            if !valid {
                return Err("invalid CPU interrupt route");
            }
            if let CpuInterruptTarget::Cpu(id) = route.target
                && !self.cpus.iter().any(|cpu| cpu.hardware_id == id)
            {
                return Err("interrupt targets an unknown CPU");
            }
            if self.cpu_interrupts[..index]
                .iter()
                .any(|other| other.controller == route.controller && other.output == route.output)
            {
                return Err("duplicate CPU interrupt output");
            }
        }
        Ok(())
    }
}
