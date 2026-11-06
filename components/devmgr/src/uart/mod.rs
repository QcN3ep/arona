// SPDX-License-Identifier: MPL-2.0

//! 16550 UART 的配置、收发与中断处理。

pub mod service;
mod session;

pub use session::{Access, PendingOpen, PendingRead, Session};

use runtime::{
    Device, Event, Handle,
    device::{InterruptBinding, Mmio},
};

use crate::{Error, device::DeviceDescription};

// REFERENCE: https://media.digikey.com/pdf/Data%20Sheets/Texas%20Instruments%20PDFs/PC16550D.pdf#page=17
const DATA: usize = 0;
const IER: usize = 1;
const IIR: usize = 2;
const FCR: usize = 2;
const LCR: usize = 3;
const MCR: usize = 4;
const LSR: usize = 5;
const MSR: usize = 6;
const DLAB: u8 = 0x80;
const RECEIVE_INTERRUPT: u8 = 0x01;
const TRANSMIT_INTERRUPT: u8 = 0x02;
const LINE_INTERRUPT: u8 = 0x04;
const DATA_READY: u8 = 0x01;
const HOLDING_EMPTY: u8 = 0x20;
const TRANSMITTER_EMPTY: u8 = 0x40;
const LINE_ERRORS: u8 = 0x1e;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataBits {
    Five,
    Six,
    Seven,
    Eight,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Parity {
    None,
    Odd,
    Even,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopBits {
    One,
    OneAndHalf,
    Two,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Settings {
    /// None 保留当前分频。
    pub baud_rate: Option<u32>,
    pub data_bits: DataBits,
    pub parity: Parity,
    pub stop_bits: StopBits,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            baud_rate: Some(115_200),
            data_bits: DataBits::Eight,
            parity: Parity::None,
            stop_bits: StopBits::One,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Configuration {
    id: u32,
    layout: RegisterLayout,
    clock_frequency_hz: Option<u64>,
}

#[derive(Clone, Copy, Debug)]
struct RegisterLayout {
    shift: u32,
    width: usize,
    offset: usize,
    endian: Endian,
}

#[derive(Clone, Copy, Debug)]
enum Endian {
    Little,
    Big,
    Native,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LineErrors {
    pub overrun: bool,
    pub parity: bool,
    pub framing: bool,
    pub break_condition: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadResult {
    pub bytes: usize,
    pub errors: LineErrors,
}

pub struct Uart<'a> {
    interrupt: InterruptBinding<'a>,
    registers: Registers<'a>,
    clock_frequency_hz: Option<u64>,
    fifo_size: usize,
    enabled_interrupts: u8,
    pending_errors: u8,
}

struct Registers<'a> {
    mmio: Mmio<'a>,
    layout: RegisterLayout,
}

pub fn probe(device: &DeviceDescription<'_>) -> Result<Option<Configuration>, Error> {
    if !device.matches("devicetree", "ns16550a")? && !device.matches("devicetree", "ns16550")? {
        return Ok(None);
    }
    let info = device.info();
    if info.mmio_regions != 1 || info.interrupts != 1 {
        return Err(Error::Unsupported);
    }
    let big = flag(device, "big-endian")?;
    let little = flag(device, "little-endian")?;
    let native = flag(device, "native-endian")?;
    if u8::from(big) + u8::from(little) + u8::from(native) > 1 {
        return Err(Error::InvalidConfiguration);
    }
    let layout = RegisterLayout {
        shift: cell(device, "reg-shift")?.unwrap_or(0),
        width: cell(device, "reg-io-width")?.unwrap_or(1) as usize,
        offset: cell(device, "reg-offset")?.unwrap_or(0) as usize,
        endian: if big {
            Endian::Big
        } else if native {
            Endian::Native
        } else {
            Endian::Little
        },
    };
    let stride = 1usize
        .checked_shl(layout.shift)
        .ok_or(Error::InvalidConfiguration)?;
    if !matches!(layout.width, 1 | 2 | 4) || stride < layout.width {
        return Err(Error::InvalidConfiguration);
    }
    let mut frequency = integer(device, "clock-frequency")?;
    if frequency.is_none()
        && let Some(phandle) = cell(device, "clocks")?
    {
        if phandle == 0 || phandle == u32::MAX {
            return Err(Error::InvalidConfiguration);
        }
        let clock = device.resolve("devicetree", "phandle", &phandle.to_be_bytes())?;
        if !clock.matches("devicetree", "fixed-clock")? || cell(&clock, "#clock-cells")? != Some(0)
        {
            return Err(Error::Unsupported);
        }
        frequency = Some(integer(&clock, "clock-frequency")?.ok_or(Error::InvalidConfiguration)?);
    }
    if frequency == Some(0) {
        return Err(Error::InvalidConfiguration);
    }
    Ok(Some(Configuration {
        id: info.id,
        layout,
        clock_frequency_hz: frequency,
    }))
}

impl<'a> Uart<'a> {
    pub fn new(
        config: Configuration,
        device: &'a Handle<Device>,
        event: &'a Handle<Event>,
        mask: u64,
        settings: Settings,
    ) -> Result<Self, Error> {
        settings.hardware(config.clock_frequency_hz)?;
        let mmio = device.map_mmio(config.id, 0)?;
        let mut registers = Registers::new(mmio, config.layout)?;
        let interrupt = device.bind_interrupt(config.id, 0, event, mask)?;
        let control = registers.read(LCR)? & !DLAB;
        registers.write(LCR, control)?;
        registers.write(IER, 0)?;
        let mut uart = Self {
            interrupt,
            registers,
            clock_frequency_hz: config.clock_frequency_hz,
            fifo_size: 1,
            enabled_interrupts: 0,
            pending_errors: 0,
        };
        uart.registers.write(FCR, 0x03)?;
        uart.configure(settings)?;
        uart.registers.write(FCR, 0x07)?;
        if uart.registers.read(IIR)? & 0xc0 == 0xc0 {
            uart.fifo_size = 16;
        }
        uart.registers.write(MCR, 0x0b)?;
        uart.set_receive_interrupts(true)?;
        Ok(uart)
    }

    pub fn configure(&mut self, settings: Settings) -> Result<(), Error> {
        let (control, divisor) = settings.hardware(self.clock_frequency_hz)?;
        let status = self.line_status()?;
        if status & TRANSMITTER_EMPTY == 0 || status & DATA_READY != 0 {
            return Err(Error::Busy);
        }
        let enabled = self.enabled_interrupts;
        self.enable_interrupts(0)?;
        let result = self.registers.configure(control, divisor);
        let restore = self.enable_interrupts(enabled);
        result?;
        restore
    }

    pub fn read(&mut self, output: &mut [u8]) -> Result<ReadResult, Error> {
        let mut bytes = 0;
        for byte in output {
            if self.line_status()? & DATA_READY == 0 {
                break;
            }
            *byte = self.registers.read(DATA)?;
            bytes += 1;
        }
        let errors = core::mem::take(&mut self.pending_errors);
        Ok(ReadResult {
            bytes,
            errors: LineErrors {
                overrun: errors & 2 != 0,
                parity: errors & 4 != 0,
                framing: errors & 8 != 0,
                break_condition: errors & 16 != 0,
            },
        })
    }

    pub fn write(&mut self, bytes: &[u8]) -> Result<usize, Error> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let mut written = 0;
        while written < bytes.len() && self.line_status()? & HOLDING_EMPTY != 0 {
            let end = bytes.len().min(written + self.fifo_size);
            for byte in &bytes[written..end] {
                self.registers.write(DATA, *byte)?;
            }
            written = end;
        }
        let enabled = if written == bytes.len() {
            self.enabled_interrupts & !TRANSMIT_INTERRUPT
        } else {
            self.enabled_interrupts | TRANSMIT_INTERRUPT
        };
        self.enable_interrupts(enabled)?;
        Ok(written)
    }

    pub fn is_transmit_empty(&mut self) -> Result<bool, Error> {
        Ok(self.line_status()? & TRANSMITTER_EMPTY != 0)
    }

    pub fn set_receive_interrupts(&mut self, enabled: bool) -> Result<(), Error> {
        let mask = RECEIVE_INTERRUPT | LINE_INTERRUPT;
        let interrupts = if enabled {
            self.enabled_interrupts | mask
        } else {
            self.enabled_interrupts & !mask
        };
        self.enable_interrupts(interrupts)
    }

    /// 接收数据处理完毕后重新使能中断。
    pub fn acknowledge_interrupt(&mut self) -> Result<(), Error> {
        if self.line_status()? & DATA_READY != 0 && self.enabled_interrupts & RECEIVE_INTERRUPT != 0
        {
            return Err(Error::Busy);
        }
        match self.registers.read(IIR)? & 0x0f {
            0 => {
                self.registers.read(MSR)?;
            }
            1 | 2 | 6 => {}
            4 | 12 => return Err(Error::Busy),
            _ => return Err(Error::Unsupported),
        }
        Ok(self.interrupt.acknowledge()?)
    }

    fn line_status(&mut self) -> Result<u8, Error> {
        let status = self.registers.read(LSR)?;
        self.pending_errors |= status & LINE_ERRORS;
        Ok(status)
    }

    fn enable_interrupts(&mut self, value: u8) -> Result<(), Error> {
        if self.enabled_interrupts != value {
            self.registers.write(IER, value)?;
            self.enabled_interrupts = value;
        }
        Ok(())
    }
}

impl Drop for Uart<'_> {
    fn drop(&mut self) {
        let _ = self.registers.write(IER, 0);
    }
}

impl Settings {
    fn hardware(self, clock: Option<u64>) -> Result<(u8, Option<u16>), Error> {
        let length = match self.data_bits {
            DataBits::Five => 0,
            DataBits::Six => 1,
            DataBits::Seven => 2,
            DataBits::Eight => 3,
        };
        let stop = match (self.data_bits, self.stop_bits) {
            (_, StopBits::One) => 0,
            (DataBits::Five, StopBits::OneAndHalf) => 4,
            (DataBits::Six | DataBits::Seven | DataBits::Eight, StopBits::Two) => 4,
            _ => return Err(Error::InvalidConfiguration),
        };
        let parity = match self.parity {
            Parity::None => 0,
            Parity::Odd => 8,
            Parity::Even => 24,
        };
        let divisor = if let Some(baud) = self.baud_rate {
            if baud == 0 {
                return Err(Error::InvalidConfiguration);
            }
            let clock = clock.ok_or(Error::ClockUnavailable)?;
            let denominator = u64::from(baud) * 16;
            let divisor = clock / denominator + u64::from(clock % denominator >= denominator / 2);
            Some(
                u16::try_from(divisor)
                    .ok()
                    .filter(|value| *value != 0)
                    .ok_or(Error::InvalidConfiguration)?,
            )
        } else {
            None
        };
        Ok((length | stop | parity, divisor))
    }
}

impl<'a> Registers<'a> {
    fn new(mmio: Mmio<'a>, layout: RegisterLayout) -> Result<Self, Error> {
        let base = mmio.range().ok_or(Error::InvalidConfiguration)?.start;
        let last = 1usize
            .checked_shl(layout.shift)
            .and_then(|stride| stride.checked_mul(7))
            .and_then(|offset| layout.offset.checked_add(offset))
            .and_then(|offset| offset.checked_add(layout.width))
            .ok_or(Error::InvalidConfiguration)?;
        if last > mmio.len() || !(base + layout.offset).is_multiple_of(layout.width) {
            return Err(Error::InvalidConfiguration);
        }
        Ok(Self { mmio, layout })
    }

    fn read(&mut self, register: usize) -> Result<u8, Error> {
        let offset = self.layout.offset + (register << self.layout.shift);
        Ok(match (self.layout.width, self.layout.endian) {
            (1, _) => self.mmio.read_u8(offset)?,
            (2, Endian::Little) => u16::from_le(self.mmio.read_u16(offset)?) as u8,
            (2, Endian::Big) => u16::from_be(self.mmio.read_u16(offset)?) as u8,
            (2, Endian::Native) => self.mmio.read_u16(offset)? as u8,
            (4, Endian::Little) => u32::from_le(self.mmio.read_u32(offset)?) as u8,
            (4, Endian::Big) => u32::from_be(self.mmio.read_u32(offset)?) as u8,
            (4, Endian::Native) => self.mmio.read_u32(offset)? as u8,
            _ => return Err(Error::InvalidConfiguration),
        })
    }

    fn write(&mut self, register: usize, value: u8) -> Result<(), Error> {
        let offset = self.layout.offset + (register << self.layout.shift);
        match (self.layout.width, self.layout.endian) {
            (1, _) => self.mmio.write_u8(offset, value)?,
            (2, Endian::Little) => self.mmio.write_u16(offset, u16::from(value).to_le())?,
            (2, Endian::Big) => self.mmio.write_u16(offset, u16::from(value).to_be())?,
            (2, Endian::Native) => self.mmio.write_u16(offset, value.into())?,
            (4, Endian::Little) => self.mmio.write_u32(offset, u32::from(value).to_le())?,
            (4, Endian::Big) => self.mmio.write_u32(offset, u32::from(value).to_be())?,
            (4, Endian::Native) => self.mmio.write_u32(offset, value.into())?,
            _ => return Err(Error::InvalidConfiguration),
        }
        Ok(())
    }

    fn configure(&mut self, control: u8, divisor: Option<u16>) -> Result<(), Error> {
        if let Some(divisor) = divisor {
            self.write(LCR, control | DLAB)?;
            let result = (|| {
                self.write(DATA, divisor as u8)?;
                self.write(IER, (divisor >> 8) as u8)
            })();
            let restore = self.write(LCR, control);
            result?;
            restore
        } else {
            self.write(LCR, control)
        }
    }
}

fn cell(device: &DeviceDescription<'_>, name: &str) -> Result<Option<u32>, Error> {
    let mut bytes = [0; 128];
    device
        .property("devicetree", name, &mut bytes)?
        .map(|value| {
            value
                .try_into()
                .map(u32::from_be_bytes)
                .map_err(|_| Error::InvalidConfiguration)
        })
        .transpose()
}

fn integer(device: &DeviceDescription<'_>, name: &str) -> Result<Option<u64>, Error> {
    let mut bytes = [0; 128];
    device
        .property("devicetree", name, &mut bytes)?
        .map(|value| match value {
            [a, b, c, d] => Ok(u32::from_be_bytes([*a, *b, *c, *d]) as u64),
            [a, b, c, d, e, f, g, h] => Ok(u64::from_be_bytes([*a, *b, *c, *d, *e, *f, *g, *h])),
            _ => Err(Error::InvalidConfiguration),
        })
        .transpose()
}

fn flag(device: &DeviceDescription<'_>, name: &str) -> Result<bool, Error> {
    let mut bytes = [0; 128];
    match device.property("devicetree", name, &mut bytes)? {
        None => Ok(false),
        Some([]) => Ok(true),
        Some(_) => Err(Error::InvalidConfiguration),
    }
}
