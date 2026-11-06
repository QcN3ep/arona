// SPDX-License-Identifier: MPL-2.0

//! RTC 的识别与 Unix 时间读取。

pub mod service;
mod session;

pub use session::Session;

use crate::{Error, device::DeviceDescription};
use runtime::{Device, Handle, device::Mmio};

// REFERENCE: https://gitlab.com/qemu-project/qemu/-/blob/v11.1.2/hw/rtc/goldfish_rtc.c
const TIME_LOW: usize = 0x00;
const TIME_HIGH: usize = 0x04;
// REFERENCE: https://gitlab.com/qemu-project/qemu/-/blob/v11.1.2/hw/rtc/ls7a_rtc.c
const TOY_READ: usize = 0x2c;
const TOY_YEAR: usize = 0x30;
const CONTROL: usize = 0x40;
const TOY_ENABLE: u32 = (1 << 11) | (1 << 8);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Timestamp {
    pub seconds: i64,
    pub nanoseconds: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct Configuration {
    id: u32,
    kind: Kind,
    big_endian: bool,
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Goldfish,
    Ls7a,
}

pub struct Rtc<'a> {
    registers: Mmio<'a>,
    configuration: Configuration,
}

pub fn probe(device: &DeviceDescription<'_>) -> Result<Option<Configuration>, Error> {
    let kind = if device.matches("devicetree", "google,goldfish-rtc")? {
        Kind::Goldfish
    } else if device.matches("devicetree", "loongson,ls7a-rtc")? {
        Kind::Ls7a
    } else {
        return Ok(None);
    };
    if device.info().mmio_regions != 1 {
        return Err(Error::InvalidConfiguration);
    }
    let mut buffer = [0; 4];
    let mut flags = [false; 3];
    for (index, name) in ["big-endian", "little-endian", "native-endian"]
        .into_iter()
        .enumerate()
    {
        if let Some(value) = device.property("devicetree", name, &mut buffer)? {
            if !value.is_empty() {
                return Err(Error::InvalidConfiguration);
            }
            flags[index] = true;
        }
    }
    if flags.into_iter().filter(|value| *value).count() > 1 {
        return Err(Error::InvalidConfiguration);
    }
    Ok(Some(Configuration {
        id: device.info().id,
        kind,
        big_endian: flags[0],
    }))
}

impl<'a> Rtc<'a> {
    pub fn new(configuration: Configuration, grant: &'a Handle<Device>) -> Result<Self, Error> {
        let mut rtc = Self {
            registers: grant.map_mmio(configuration.id, 0)?,
            configuration,
        };
        let length = match configuration.kind {
            Kind::Goldfish => TIME_HIGH + 4,
            Kind::Ls7a => CONTROL + 4,
        };
        if rtc.registers.len() < length {
            return Err(Error::InvalidConfiguration);
        }
        if let Kind::Ls7a = configuration.kind {
            let control = rtc.read(CONTROL)?;
            rtc.write(CONTROL, control | TOY_ENABLE)?;
        }
        rtc.now()?;
        Ok(rtc)
    }

    pub fn now(&mut self) -> Result<Timestamp, Error> {
        match self.configuration.kind {
            Kind::Goldfish => {
                // 读取低位时锁存高位。
                let low = self.read(TIME_LOW)?;
                let high = self.read(TIME_HIGH)?;
                let nanoseconds = ((u64::from(high) << 32) | u64::from(low)) as i64;
                Ok(Timestamp {
                    seconds: nanoseconds.div_euclid(1_000_000_000),
                    nanoseconds: nanoseconds.rem_euclid(1_000_000_000) as u32,
                })
            }
            Kind::Ls7a => {
                for _ in 0..8 {
                    let calendar = self.read(TOY_READ)?;
                    let year = self.read(TOY_YEAR)?;
                    if calendar == self.read(TOY_READ)? {
                        return calendar_time(year, calendar);
                    }
                }
                Err(Error::Busy)
            }
        }
    }

    fn read(&mut self, offset: usize) -> Result<u32, Error> {
        let value = self.registers.read_u32(offset)?;
        Ok(if self.configuration.big_endian {
            u32::from_be(value)
        } else {
            u32::from_le(value)
        })
    }

    fn write(&mut self, offset: usize, value: u32) -> Result<(), Error> {
        let value = if self.configuration.big_endian {
            value.to_be()
        } else {
            value.to_le()
        };
        Ok(self.registers.write_u32(offset, value)?)
    }
}

fn calendar_time(year: u32, calendar: u32) -> Result<Timestamp, Error> {
    let year = i64::from(year as i32) + 1900;
    let month = (calendar >> 26) & 0x3f;
    let day = (calendar >> 21) & 0x1f;
    let hour = (calendar >> 16) & 0x1f;
    let minute = (calendar >> 10) & 0x3f;
    let second = (calendar >> 4) & 0x3f;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let months = [
        31,
        28 + u32::from(leap),
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if year < 1
        || !(1..=12).contains(&month)
        || day == 0
        || day > months[(month - 1) as usize]
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err(Error::DeviceFault);
    }
    let previous = year - 1;
    let days = 365 * (year - 1970) + previous / 4 - 1969 / 4 - previous / 100
        + 1969 / 100
        + previous / 400
        - 1969 / 400
        + i64::from(months[..(month - 1) as usize].iter().sum::<u32>() + day - 1);
    Ok(Timestamp {
        seconds: days * 86_400 + i64::from(hour * 3600 + minute * 60 + second),
        nanoseconds: 0,
    })
}
