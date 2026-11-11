// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use core::arch::asm;

use kernel::{
    CpuInterruptTarget, HardwareDeviceInstanceId, HardwareInfo, HardwareInterruptInfo,
    InterruptPolarity, InterruptTrigger,
    hardware::{
        interrupt::{InterruptClaim, InterruptController, InterruptError, InterruptSource},
        mmio::MmioMapper,
    },
};

use crate::{
    device::mmio::Mmio,
    interrupt::{Claims, LocalController},
};

// REFERENCE: https://github.com/riscv/riscv-plic-spec/blob/1.0.0/riscv-plic.adoc
const ENABLE: u64 = 0x2000;
const CONTEXT: u64 = 0x20_0000;

crate::declare_hardware!(
    static CONTROLLER: PlicController = PlicController { state: LocalController::new() }
        => kernel::hardware::interrupt::bind
);

struct PlicController {
    state: LocalController<Plic>,
}

struct Plic {
    id: HardwareDeviceInstanceId,
    registers: Mmio,
    lines: u32,
    enable: u64,
    context: u64,
    claims: Claims,
}

impl InterruptController for PlicController {
    fn initialize(
        &self,
        hardware: &HardwareInfo,
        cpu_id: u64,
        mmio: &dyn MmioMapper,
    ) -> Result<(), InterruptError> {
        self.state.initialize(|| {
            let mut plic = Plic::new(hardware, cpu_id, mmio)?;
            plic.initialize()?;
            Ok(plic)
        })
    }

    fn mask(&self, source: InterruptSource) -> Result<(), InterruptError> {
        self.state.with(|plic| {
            plic.validate(source)?;
            plic.set_enabled(source.line, false)
        })
    }

    fn unmask(&self, interrupt: HardwareInterruptInfo) -> Result<(), InterruptError> {
        self.state.with(|plic| {
            let source = interrupt.into();
            plic.validate(source)?;
            if plic.claims.contains(source) {
                return Err(InterruptError::Busy);
            }
            if interrupt
                .trigger
                .is_some_and(|trigger| trigger != InterruptTrigger::Level)
                || interrupt
                    .polarity
                    .is_some_and(|polarity| polarity != InterruptPolarity::ActiveHigh)
            {
                return Err(InterruptError::Unsupported);
            }
            plic.set_enabled(interrupt.line, true)
        })
    }

    fn claim(&self) -> Result<Option<InterruptClaim>, InterruptError> {
        self.state.with(|plic| {
            plic.claims.available()?;
            let registers = &plic.registers;
            let line = registers.read32(plic.context + 4)?;
            if line == 0 {
                return Ok(None);
            }
            let source = InterruptSource {
                controller: plic.id,
                line,
            };
            if plic.validate(source).is_err() {
                registers.write32(plic.context + 4, line)?;
                // SAFETY: The current processor must exclusively own the supervisor external interrupt enable bit.
                unsafe { asm!("csrc sie, {}", in(reg) 1usize << 9, options(nostack)) };
                return Err(InterruptError::InvalidSource);
            }
            let token = plic.claims.begin(source);
            Ok(Some(InterruptClaim::new(source, token, complete)))
        })
    }
}

impl Plic {
    fn new(
        hardware: &HardwareInfo,
        cpu_id: u64,
        mmio: &dyn MmioMapper,
    ) -> Result<Self, InterruptError> {
        let mut controllers = hardware.devices.iter().filter(|device| {
            device.has_identity("devicetree", "riscv,plic0")
                || device.has_identity("devicetree", "sifive,plic-1.0.0")
        });
        let device = controllers.next().ok_or(InterruptError::Unsupported)?;
        if controllers.next().is_some() {
            return Err(InterruptError::Unsupported);
        }
        let lines = device
            .interrupt_lines
            .as_ref()
            .ok_or(InterruptError::InvalidTopology)?;
        if lines.start != 1 || !(2..=1024).contains(&lines.end) {
            return Err(InterruptError::InvalidTopology);
        }
        let mut routes = hardware.cpu_interrupts.iter().filter(|route| {
            route.controller == device.instance_id
                && route.target == CpuInterruptTarget::Cpu(cpu_id)
                && route.line == 9
        });
        let route = routes.next().ok_or(InterruptError::InvalidTopology)?;
        if routes.next().is_some() || route.output >= 15872 {
            return Err(InterruptError::InvalidTopology);
        }
        let context = CONTEXT + route.output as u64 * 0x1000;
        let enable = ENABLE + route.output as u64 * 0x80;
        let range = *device
            .mmio_regions
            .first()
            .ok_or(InterruptError::InvalidTopology)?;
        if range
            .end
            .checked_sub(range.start)
            .is_none_or(|length| length < context + 8)
        {
            return Err(InterruptError::InvalidTopology);
        }
        Ok(Self {
            id: device.instance_id,
            registers: Mmio::new(mmio.map(device.instance_id, 0)?)?,
            lines: lines.end,
            enable,
            context,
            claims: Claims::default(),
        })
    }

    fn initialize(&mut self) -> Result<(), InterruptError> {
        let registers = &self.registers;
        registers.write32(self.context, u32::MAX)?;
        for word in 0..self.lines.div_ceil(32) {
            registers.write32(self.enable + word as u64 * 4, 0)?;
        }
        for line in 1..self.lines {
            registers.write32(line as u64 * 4, 0)?;
        }
        registers.write32(self.context, 0)?;
        // SAFETY: The exception entry must be ready, and the current processor must exclusively own the supervisor external interrupt enable bit.
        unsafe { asm!("csrs sie, {}", in(reg) 1usize << 9, options(nostack)) };
        Ok(())
    }

    fn validate(&self, source: InterruptSource) -> Result<(), InterruptError> {
        if source.controller != self.id || source.line == 0 || source.line >= self.lines {
            return Err(InterruptError::InvalidSource);
        }
        Ok(())
    }

    fn set_enabled(&self, line: u32, enabled: bool) -> Result<(), InterruptError> {
        let registers = &self.registers;
        let offset = self.enable + (line / 32) as u64 * 4;
        let bit = 1 << (line % 32);
        let value = registers.read32(offset)?;
        registers.write32(offset, if enabled { value | bit } else { value & !bit })?;
        registers.write32(line as u64 * 4, u32::from(enabled))?;
        Ok(())
    }

    fn complete(
        &mut self,
        source: InterruptSource,
        token: usize,
        abandoned: bool,
    ) -> Result<(), InterruptError> {
        self.claims.validate(source, token)?;
        let registers = &self.registers;
        let offset = self.enable + (source.line / 32) as u64 * 4;
        let bit = 1 << (source.line % 32);
        let enabled = registers.read32(offset)?;
        if abandoned {
            registers.write32(source.line as u64 * 4, 0)?;
        }
        // The PLIC only accepts completion writes for inputs still enabled in the current context.
        registers.write32(offset, enabled | bit)?;
        registers.write32(self.context + 4, source.line)?;
        registers.write32(offset, if abandoned { enabled & !bit } else { enabled })?;
        self.claims.finish();
        Ok(())
    }
}

fn complete(source: InterruptSource, token: usize, abandoned: bool) -> Result<(), InterruptError> {
    CONTROLLER
        .state
        .with(|plic| plic.complete(source, token, abandoned))
}
