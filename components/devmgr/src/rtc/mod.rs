// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! RTC identification and Unix time reads and writes.

pub mod service;
mod session;

pub use session::Session;

use crate::{Error, device::DeviceDescription};
use runtime::{Device, Handle, device::Mmio};

// REFERENCE: https://gitlab.com/qemu-project/qemu/-/blob/v11.1.2/hw/rtc/goldfish_rtc.c
const TIME_LOW: usize = 0x00;
const TIME_HIGH: usize = 0x04;
// REFERENCE: https://gitlab.com/qemu-project/qemu/-/blob/v11.1.2/hw/rtc/ls7a_rtc.c
const TOY_WRITE: usize = 0x24;
const TOY_YEAR_WRITE: usize = 0x28;
const TOY_READ: usize = 0x2c;
const TOY_YEAR: usize = 0x30;
const CONTROL: usize = 0x40;
const TOY_ENABLE: u32 = (1 << 11) | (1 << 8);
const NANOSECONDS_PER_SECOND: i64 = 1_000_000_000;
const MIN_YEAR: i64 = 1970;
const MAX_YEAR: i64 = 2100;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Timestamp {
    pub seconds: i64,
    pub nanoseconds: u32,
}

/// Calendar fields used by the LS7A registers and the Linux `rtc_time` structure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Calendar {
    pub year: i32,
    /// Month in the range 1 to 12.
    pub month: u8,
    /// Day of the month in the range 1 to 31.
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    /// Days since Sunday, in the range 0 to 6.
    pub weekday: u8,
    /// Days since January 1, in the range 0 to 365.
    pub year_day: u16,
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

impl Timestamp {
    /// Converts calendar fields into Unix time in UTC.
    ///
    /// The accepted year range matches the representable range of both backends and keeps
    /// malformed or wrapped dates away from the registers.
    pub fn from_calendar(
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
    ) -> Result<Self, Error> {
        if !(MIN_YEAR..=MAX_YEAR).contains(&i64::from(year))
            || !(1..=12).contains(&month)
            || day == 0
            || day > days_in_month(year, month)
            || hour > 23
            || minute > 59
            || second > 59
        {
            return Err(Error::InvalidRequest);
        }
        let days = days_from_civil(year, month, day);
        Ok(Self {
            seconds: days * 86_400 + i64::from(hour * 3_600 + minute * 60 + second),
            nanoseconds: 0,
        })
    }

    /// Converts Unix time in UTC into calendar fields.
    pub fn calendar(self) -> Result<Calendar, Error> {
        if self.seconds < 0 {
            return Err(Error::InvalidRequest);
        }
        let days = self.seconds.div_euclid(86_400);
        let day_seconds = self.seconds.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        if !(MIN_YEAR..=MAX_YEAR).contains(&year) {
            return Err(Error::InvalidRequest);
        }
        let year = year as i32;
        let year_day = days - days_from_civil(year, 1, 1);
        Ok(Calendar {
            year,
            month: month as u8,
            day: day as u8,
            hour: (day_seconds / 3_600) as u8,
            minute: (day_seconds % 3_600 / 60) as u8,
            second: (day_seconds % 60) as u8,
            weekday: (days + 4).rem_euclid(7) as u8,
            year_day: year_day as u16,
        })
    }
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
                // Reading the low word latches the high word.
                let low = self.read(TIME_LOW)?;
                let high = self.read(TIME_HIGH)?;
                let nanoseconds = ((u64::from(high) << 32) | u64::from(low)) as i64;
                Ok(Timestamp {
                    seconds: nanoseconds.div_euclid(NANOSECONDS_PER_SECOND),
                    nanoseconds: nanoseconds.rem_euclid(NANOSECONDS_PER_SECOND) as u32,
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

    /// Writes Unix time in UTC to the RTC and verifies that the registers accepted it.
    pub fn set(&mut self, timestamp: Timestamp) -> Result<(), Error> {
        if timestamp.nanoseconds >= NANOSECONDS_PER_SECOND as u32 {
            return Err(Error::InvalidRequest);
        }
        let calendar = timestamp.calendar()?;
        match self.configuration.kind {
            Kind::Goldfish => {
                let seconds =
                    u64::try_from(timestamp.seconds).map_err(|_| Error::InvalidRequest)?;
                let nanoseconds = seconds
                    .checked_mul(NANOSECONDS_PER_SECOND as u64)
                    .and_then(|value| value.checked_add(u64::from(timestamp.nanoseconds)))
                    .ok_or(Error::InvalidRequest)?;
                self.write(TIME_LOW, nanoseconds as u32)?;
                self.write(TIME_HIGH, (nanoseconds >> 32) as u32)?;
            }
            Kind::Ls7a => {
                if timestamp.nanoseconds != 0 {
                    return Err(Error::Unsupported);
                }
                let year =
                    u32::try_from(calendar.year - 1900).map_err(|_| Error::InvalidRequest)?;
                let value = u32::from(calendar.second) << 4
                    | u32::from(calendar.minute) << 10
                    | u32::from(calendar.hour) << 16
                    | u32::from(calendar.day) << 21
                    | u32::from(calendar.month) << 26;
                // The year goes first so a leap-day target cannot be normalized by the old year.
                self.write(TOY_YEAR_WRITE, year)?;
                self.write(TOY_WRITE, value)?;
            }
        }
        let now = self.now()?;
        if now.seconds == timestamp.seconds || now.seconds == timestamp.seconds + 1 {
            Ok(())
        } else {
            Err(Error::DeviceFault)
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

fn is_leap(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

// REFERENCE: https://howardhinnant.github.io/date_algorithms.html
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * i64::from((month + 9) % 12) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    (year + i64::from(month <= 2), month as u32, day as u32)
}

fn calendar_time(year: u32, calendar: u32) -> Result<Timestamp, Error> {
    let year = i64::from(year) + 1900;
    let year = i32::try_from(year).map_err(|_| Error::DeviceFault)?;
    Timestamp::from_calendar(
        year,
        (calendar >> 26) & 0x3f,
        (calendar >> 21) & 0x1f,
        (calendar >> 16) & 0x1f,
        (calendar >> 10) & 0x3f,
        (calendar >> 4) & 0x3f,
    )
    .map_err(|_| Error::DeviceFault)
}
