// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use core::fmt;

use arch::device::uart::Uart;

pub fn run(uart: &Uart) {
    struct Reentrant<'a>(&'a Uart);
    impl fmt::Display for Reentrant<'_> {
        fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
            assert!(self.0.write(format_args!("nested")).is_err());
            assert!(self.0.flush().is_err());
            self.0.emergency_write(format_args!("emergency output\n"))?;
            output.write_str("outer output\n")
        }
    }

    struct InvalidFormat;
    impl fmt::Display for InvalidFormat {
        fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
            Err(fmt::Error)
        }
    }

    uart.write(format_args!("{}", Reentrant(uart))).unwrap();
    assert!(uart.write(format_args!("{InvalidFormat}")).is_err());
    uart.write(format_args!("recovered output\n")).unwrap();
    uart.flush().unwrap();
    let bytes = [b'x'; 1025];
    let oversized = core::str::from_utf8(&bytes).unwrap();
    assert!(uart.write(format_args!("{oversized}")).is_err());
    assert!(uart.emergency_write(format_args!("{oversized}")).is_err());
    uart.write(format_args!("\nUART tests passed\n")).unwrap();
    uart.flush().unwrap();
}
