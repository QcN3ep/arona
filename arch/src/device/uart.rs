// SPDX-License-Identifier: MPL-2.0

//! Bounded polling output for the 16550 UART.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, Ordering};

use super::io_fence;

const MAX_BYTES: usize = 1024;
const MAX_POLLS: usize = 16384;

pub struct Uart {
    base: usize,
    writing: AtomicBool,
}

impl Uart {
    /// # Safety
    ///
    /// base must point to persistently accessible 16550 registers with byte width and stride of 1; normal access must be serialized through this instance.
    pub const unsafe fn new(base: usize) -> Self {
        Self {
            base,
            writing: AtomicBool::new(false),
        }
    }

    pub fn init(&self) -> fmt::Result {
        let _guard = self.try_lock()?;
        self.write_register(3, 3);
        self.write_register(1, 0);
        Ok(())
    }

    pub fn write(&self, arguments: fmt::Arguments<'_>) -> fmt::Result {
        let _guard = self.try_lock()?;
        self.write_unlocked(arguments)
    }

    pub fn flush(&self) -> fmt::Result {
        let _guard = self.try_lock()?;
        for _ in 0..MAX_POLLS {
            if self.read_register(5) & 0x40 != 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(fmt::Error)
    }

    /// Bypass the output lock, allowing output during a panic to interleave with unfinished normal output.
    pub fn emergency_write(&self, arguments: fmt::Arguments<'_>) -> fmt::Result {
        self.write_unlocked(arguments)
    }

    fn write_unlocked(&self, arguments: fmt::Arguments<'_>) -> fmt::Result {
        Writer {
            uart: self,
            bytes_left: MAX_BYTES,
            polls_left: MAX_POLLS,
        }
        .write_fmt(arguments)
    }

    fn try_lock(&self) -> Result<WriteGuard<'_>, fmt::Error> {
        self.writing
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map_err(|_| fmt::Error)?;
        Ok(WriteGuard(self))
    }

    fn read_register(&self, offset: usize) -> u8 {
        // SAFETY: The address must lie within persistently readable UART registers.
        let value = unsafe { ((self.base + offset) as *const u8).read_volatile() };
        io_fence();
        value
    }

    fn write_register(&self, offset: usize, value: u8) {
        io_fence();
        // SAFETY: The address must lie within persistently writable UART registers.
        unsafe { ((self.base + offset) as *mut u8).write_volatile(value) };
        io_fence();
    }
}

struct WriteGuard<'a>(&'a Uart);

impl Drop for WriteGuard<'_> {
    fn drop(&mut self) {
        self.0.writing.store(false, Ordering::Release);
    }
}

struct Writer<'a> {
    uart: &'a Uart,
    bytes_left: usize,
    polls_left: usize,
}

impl Writer<'_> {
    fn write_byte(&mut self, byte: u8) -> fmt::Result {
        if self.bytes_left == 0 {
            return Err(fmt::Error);
        }
        while self.polls_left > 0 {
            self.polls_left -= 1;
            if self.uart.read_register(5) & 0x20 != 0 {
                self.uart.write_register(0, byte);
                self.bytes_left -= 1;
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(fmt::Error)
    }
}

impl Write for Writer<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for byte in text.bytes() {
            if byte == b'\n' {
                self.write_byte(b'\r')?;
            }
            self.write_byte(byte)?;
        }
        Ok(())
    }
}
