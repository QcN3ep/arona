// SPDX-License-Identifier: MPL-2.0

//! Early UART log sink and panic output.

use core::fmt;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};

use kernel::{log::Record, logger::Sink};

use crate::device::uart::Uart;

pub(crate) struct EarlyConsole {
    uart: Uart,
    panicking: AtomicBool,
}

impl EarlyConsole {
    pub const fn new(uart: Uart) -> Self {
        Self {
            uart,
            panicking: AtomicBool::new(false),
        }
    }

    pub fn init(&self) {
        self.uart.init().expect("failed to initialize early UART");
    }

    pub fn panic(&self, info: &PanicInfo<'_>) {
        if !self.panicking.swap(true, Ordering::Relaxed) {
            let _ = self.uart.emergency_write(format_args!("\nPANIC: {info}\n"));
        }
    }

    pub fn emergency_write(&self, arguments: fmt::Arguments<'_>) {
        let _ = self.uart.emergency_write(arguments);
    }
}

impl Sink for EarlyConsole {
    fn write(&self, record: &Record<'_>) -> fmt::Result {
        self.uart.write(format_args!(
            "{:<5} {}: {}\n",
            record.level(),
            record.target(),
            record.args()
        ))
    }

    fn flush(&self) -> fmt::Result {
        self.uart.flush()
    }
}
