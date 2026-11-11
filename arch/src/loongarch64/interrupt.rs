// SPDX-License-Identifier: MPL-2.0

use core::arch::asm;

use kernel::{
    CpuInterruptTarget, HardwareDeviceInfo, HardwareDeviceInstanceId, HardwareDeviceLocation,
    HardwareInfo, HardwareInterruptInfo, InterruptPolarity, InterruptTrigger,
    hardware::{
        interrupt::{InterruptClaim, InterruptController, InterruptError, InterruptSource},
        mmio::MmioMapper,
    },
};

use crate::{
    device::mmio::Mmio,
    interrupt::{Claims, LocalController},
};

// REFERENCE: https://loongson.github.io/LoongArch-Documentation/Loongson-3A5000-usermanual-EN.html#extended-io-interrupts
const NODEMAP: usize = 0x14a0;
const IPMAP: usize = 0x14c0;
const ENABLE: usize = 0x1600;
const BOUNCE: usize = 0x1680;
const ISR: usize = 0x1800;
const COREMAP: usize = 0x1c00;

// REFERENCE: https://github.com/torvalds/linux/blob/v6.18/drivers/irqchip/irq-loongson-pch-pic.c
const PIC_MASK: u64 = 0x20;
const PIC_HTMSI: u64 = 0x40;
const PIC_EDGE: u64 = 0x60;
const PIC_CLEAR: u64 = 0x80;
const PIC_POLARITY: u64 = 0x3e0;

crate::declare_hardware!(
    static CONTROLLER: EiointcController = EiointcController { state: LocalController::new() }
        => kernel::hardware::interrupt::bind
);

struct EiointcController {
    state: LocalController<Eiointc>,
}

struct Eiointc {
    id: HardwareDeviceInstanceId,
    pin: u32,
    cpu: u32,
    pic: PchPic,
    claims: Claims,
}

struct PchPic {
    id: HardwareDeviceInstanceId,
    registers: Mmio,
    vector_base: u32,
    lines: u32,
}

impl InterruptController for EiointcController {
    fn initialize(
        &self,
        hardware: &HardwareInfo,
        cpu_id: u64,
        mmio: &dyn MmioMapper,
    ) -> Result<(), InterruptError> {
        self.state.initialize(|| {
            let mut controller = Eiointc::new(hardware, cpu_id, mmio)?;
            controller.initialize()?;
            Ok(controller)
        })
    }

    fn mask(&self, source: InterruptSource) -> Result<(), InterruptError> {
        self.state.with(|controller| controller.mask(source))
    }

    fn unmask(&self, interrupt: HardwareInterruptInfo) -> Result<(), InterruptError> {
        self.state.with(|controller| {
            let source = interrupt.into();
            let vector = controller.vector(source)?;
            if controller.claims.contains(source) {
                return Err(InterruptError::Busy);
            }
            if source.controller == controller.pic.id {
                controller.pic.configure(interrupt)?;
                set_bit(ENABLE, vector, true);
                controller.pic.set_masked(source.line, false)?;
            } else {
                if interrupt.trigger.is_some() || interrupt.polarity.is_some() {
                    return Err(InterruptError::Unsupported);
                }
                set_bit(ENABLE, vector, true);
            }
            Ok(())
        })
    }

    fn claim(&self) -> Result<Option<InterruptClaim>, InterruptError> {
        self.state.with(|controller| {
            controller.claims.available()?;
            for word in 0..8 {
                let pending = read_iocsr(ISR + word * 4) & read_iocsr(ENABLE + word * 4);
                if pending == 0 {
                    continue;
                }
                let vector = word as u32 * 32 + pending.trailing_zeros();
                let source = if (controller.pic.vector_base
                    ..controller.pic.vector_base + controller.pic.lines)
                    .contains(&vector)
                {
                    InterruptSource {
                        controller: controller.pic.id,
                        line: vector - controller.pic.vector_base,
                    }
                } else {
                    InterruptSource {
                        controller: controller.id,
                        line: vector,
                    }
                };
                let token = controller.claims.begin(source);
                return Ok(Some(InterruptClaim::new(source, token, complete)));
            }
            Ok(None)
        })
    }
}

impl Eiointc {
    fn new(
        hardware: &HardwareInfo,
        cpu_id: u64,
        mmio: &dyn MmioMapper,
    ) -> Result<Self, InterruptError> {
        if cpu_id >= 4 {
            return Err(InterruptError::Unsupported);
        }
        let device = find(hardware, "loongson,ls2k2000-eiointc")?;
        if !matches!(&device.location, HardwareDeviceLocation::Iocsr { registers } if registers.start == 0x1400 && registers.end >= 0x1c00)
            || device.interrupt_lines != Some(0..256)
        {
            return Err(InterruptError::InvalidTopology);
        }
        let mut routes = hardware.cpu_interrupts.iter().filter(|route| {
            route.controller == device.instance_id
                && (route.target == CpuInterruptTarget::AllCpus
                    || route.target == CpuInterruptTarget::Cpu(cpu_id))
        });
        let route = routes.next().ok_or(InterruptError::InvalidTopology)?;
        if routes.next().is_some() || !(2..=5).contains(&route.line) {
            return Err(InterruptError::Unsupported);
        }
        let pic = find(hardware, "loongson,pch-pic-1.0")?;
        let parent = pic.interrupt_route.ok_or(InterruptError::InvalidTopology)?;
        if parent.controller != device.instance_id
            || parent.base_line > 192
            || pic.interrupt_lines != Some(0..64)
        {
            return Err(InterruptError::InvalidTopology);
        }
        let range = *pic
            .mmio_regions
            .first()
            .ok_or(InterruptError::InvalidTopology)?;
        if range
            .end
            .checked_sub(range.start)
            .is_none_or(|length| length < 0x400)
        {
            return Err(InterruptError::InvalidTopology);
        }
        let registers = Mmio::new(mmio.map(pic.instance_id, 0)?)?;
        let lines = ((registers.read32(4)? >> 16) & 0xff) + 1;
        if lines > 64 {
            return Err(InterruptError::Unsupported);
        }
        Ok(Self {
            id: device.instance_id,
            pin: route.line,
            cpu: cpu_id as u32,
            pic: PchPic {
                id: pic.instance_id,
                registers,
                vector_base: parent.base_line,
                lines,
            },
            claims: Claims::default(),
        })
    }

    fn initialize(&mut self) -> Result<(), InterruptError> {
        for word in 0..8 {
            write_iocsr(ENABLE + word * 4, 0);
            write_iocsr(BOUNCE + word * 4, 0);
            write_iocsr(ISR + word * 4, u32::MAX);
            write_iocsr(
                NODEMAP + word * 4,
                (1 << (word * 2)) | (1 << (word * 2 + 1 + 16)),
            );
        }
        let pins = (1 << (self.pin - 2)) * 0x0101_0101;
        for word in 0..2 {
            write_iocsr(IPMAP + word * 4, pins);
        }
        for word in 0..64 {
            write_iocsr(COREMAP + word * 4, (1 << self.cpu) * 0x0101_0101);
        }
        self.pic.initialize()?;
        // SAFETY: The current processor must exclusively own the EIOINTC configuration and the corresponding external interrupt bit in ECFG.
        unsafe {
            let misc: usize;
            asm!("iocsrrd.d {}, {}", out(reg) misc, in(reg) 0x420usize, options(nostack));
            asm!("iocsrwr.d {}, {}", in(reg) misc | (1 << 48), in(reg) 0x420usize, options(nostack));
            asm!("csrxchg {value}, {mask}, 0x4", value = inout(reg) 1usize << self.pin => _, mask = in(reg) 1usize << self.pin, options(nostack));
        }
        Ok(())
    }

    fn vector(&self, source: InterruptSource) -> Result<u32, InterruptError> {
        if source.controller == self.pic.id && source.line < self.pic.lines {
            return Ok(self.pic.vector_base + source.line);
        }
        if source.controller == self.id
            && source.line < 256
            && !(self.pic.vector_base..self.pic.vector_base + self.pic.lines).contains(&source.line)
        {
            return Ok(source.line);
        }
        Err(InterruptError::InvalidSource)
    }

    fn mask(&self, source: InterruptSource) -> Result<(), InterruptError> {
        let vector = self.vector(source)?;
        if source.controller == self.pic.id {
            self.pic.set_masked(source.line, true)?;
        }
        set_bit(ENABLE, vector, false);
        Ok(())
    }

    fn complete(
        &mut self,
        source: InterruptSource,
        token: usize,
        abandoned: bool,
    ) -> Result<(), InterruptError> {
        self.claims.validate(source, token)?;
        let vector = self.vector(source)?;
        if abandoned {
            self.mask(source)?;
        }
        if source.controller == self.pic.id {
            let registers = &self.pic.registers;
            let offset = (source.line / 32) as u64 * 4;
            let bit = 1 << (source.line % 32);
            if registers.read32(PIC_EDGE + offset)? & bit != 0 {
                registers.write32(PIC_CLEAR + offset, bit)?;
            }
        }
        write_iocsr(ISR + (vector / 32) as usize * 4, 1 << (vector % 32));
        // Re-sample level inputs that are still asserted when re-enabling.
        if read_iocsr(ENABLE + (vector / 32) as usize * 4) & (1 << (vector % 32)) != 0 {
            set_bit(ENABLE, vector, false);
            set_bit(ENABLE, vector, true);
        }
        self.claims.finish();
        Ok(())
    }
}

impl PchPic {
    fn initialize(&self) -> Result<(), InterruptError> {
        let registers = &self.registers;
        for word in 0..2 {
            let offset = word * 4;
            registers.write32(PIC_MASK + offset, u32::MAX)?;
            registers.write32(PIC_EDGE + offset, 0)?;
            registers.write32(PIC_CLEAR + offset, u32::MAX)?;
            registers.write32(PIC_HTMSI + offset, u32::MAX)?;
            registers.write32(PIC_POLARITY + offset, 0)?;
            registers.write32(0xc0 + offset, 0)?;
            registers.write32(0xe0 + offset, 0)?;
        }
        for line in 0..self.lines {
            registers.write8(0x100 + line as u64, 1)?;
            registers.write8(0x200 + line as u64, (self.vector_base + line) as u8)?;
        }
        Ok(())
    }

    fn configure(&self, interrupt: HardwareInterruptInfo) -> Result<(), InterruptError> {
        let edge = match interrupt.trigger {
            Some(InterruptTrigger::Edge) => true,
            Some(InterruptTrigger::Level) => false,
            None => return Err(InterruptError::Unsupported),
        };
        let low = match interrupt.polarity {
            Some(InterruptPolarity::ActiveLow) => true,
            Some(InterruptPolarity::ActiveHigh) => false,
            None => return Err(InterruptError::Unsupported),
        };
        let registers = &self.registers;
        pic_bit(registers, PIC_MASK, interrupt.line, true)?;
        pic_bit(registers, PIC_EDGE, interrupt.line, edge)?;
        pic_bit(registers, PIC_POLARITY, interrupt.line, low)
    }

    fn set_masked(&self, line: u32, masked: bool) -> Result<(), InterruptError> {
        pic_bit(&self.registers, PIC_MASK, line, masked)
    }
}

fn find<'a>(
    hardware: &'a HardwareInfo,
    compatible: &str,
) -> Result<&'a HardwareDeviceInfo, InterruptError> {
    let mut devices = hardware
        .devices
        .iter()
        .filter(|device| device.has_identity("devicetree", compatible));
    let device = devices.next().ok_or(InterruptError::Unsupported)?;
    if devices.next().is_some() {
        return Err(InterruptError::Unsupported);
    }
    Ok(device)
}

fn pic_bit(registers: &Mmio, base: u64, line: u32, set: bool) -> Result<(), InterruptError> {
    let offset = base + (line / 32) as u64 * 4;
    let bit = 1 << (line % 32);
    let value = registers.read32(offset)?;
    registers.write32(offset, if set { value | bit } else { value & !bit })?;
    Ok(())
}

fn read_iocsr(address: usize) -> u32 {
    let value: usize;
    // SAFETY: The address must be a persistently readable EIOINTC IOCSR register.
    unsafe { asm!("iocsrrd.w {}, {}", out(reg) value, in(reg) address, options(nostack)) };
    value as u32
}

fn write_iocsr(address: usize, value: u32) {
    // SAFETY: The address must be an exclusively writable EIOINTC IOCSR register.
    unsafe { asm!("iocsrwr.w {}, {}", in(reg) value as usize, in(reg) address, options(nostack)) };
}

fn set_bit(base: usize, vector: u32, set: bool) {
    let address = base + (vector / 32) as usize * 4;
    let bit = 1 << (vector % 32);
    let value = read_iocsr(address);
    write_iocsr(address, if set { value | bit } else { value & !bit });
}

fn complete(source: InterruptSource, token: usize, abandoned: bool) -> Result<(), InterruptError> {
    CONTROLLER
        .state
        .with(|controller| controller.complete(source, token, abandoned))
}
